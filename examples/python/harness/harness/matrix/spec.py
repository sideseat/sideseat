"""``versions.toml``: which historical releases a suite is replayed against, and how they are installed."""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

#: A variant's name becomes a directory name on every platform the fixtures are checked out on.
_NAME = re.compile(r"^[0-9A-Za-z][0-9A-Za-z._+-]*$")
FILE = "versions.toml"
CENSUS = "versions.census.json"


@dataclass(frozen=True)
class Variant:
    """One historical release under one telemetry profile, captured as its own fixture mode."""

    version: str
    profile: str
    #: Further pins this release needs to resolve beside the harness, each with its reason in ``doc``.
    also: tuple[str, ...]
    #: The scenarios this variant records; the matrix's own list unless the release cannot run some.
    scenarios: tuple[str, ...]
    doc: str
    #: The release the suite's own lockfile holds: its fixtures are the ordinary capture (mode ``native``),
    #: recorded by ``capture`` rather than by the matrix.
    current: bool = False

    @property
    def name(self) -> str:
        return (
            self.version
            if self.profile == "default"
            else f"{self.version}+{self.profile}"
        )

    def mode(self, mode: str) -> str:
        """The fixture mode directory: ``native@1.20.0``, ``native@1.20.0+semconv-latest``."""
        return mode if self.current else f"{mode}@{self.name}"


@dataclass(frozen=True)
class Matrix:
    suite: Path
    #: The distribution whose release history defines the support window.
    package: str
    #: The requirement installed for a release, ``{version}`` substituted.
    pin: str
    #: First day of the support window; the census covers every release from it on.
    since: str
    #: Every environment resolves as of this instant (``uv --exclude-newer``), so a variant installs the
    #: same dependency set whenever it is rebuilt without a lockfile per variant.
    resolved_before: str
    python: str
    #: The scenarios the census runs on every release; together they should exercise every rule family
    #: the producer's telemetry reaches, because a release can change one carrier and keep the others.
    probes: tuple[str, ...]
    scenarios: tuple[str, ...]
    profiles: dict[str, dict[str, str]]
    prereleases: bool
    modes: tuple[str, ...]
    variants: tuple[Variant, ...] = field(default=())
    #: Releases the census could not run, each with the reviewed reason and the date it must be revisited.
    exemptions: dict[str, tuple[str, str]] = field(default_factory=dict)

    def requirements(self, variant: Variant) -> list[str]:
        return [self.pin.format(version=variant.version), *variant.also]

    def variant(self, name: str) -> Variant:
        for variant in self.variants:
            if variant.name == name:
                return variant
        known = ", ".join(v.name for v in self.variants)
        raise SystemExit(f"{self.suite.name}: no variant {name!r}; known: {known}")


class MatrixError(ValueError):
    """A ``versions.toml`` that cannot be followed, with the reason."""


def load(suite: Path) -> Matrix | None:
    """The suite's matrix, or ``None`` if it has none."""
    path = suite / FILE
    if not path.exists():
        return None
    return parse(path.read_text(), suite)


def parse(text: str, suite: Path) -> Matrix:
    document = tomllib.loads(text)
    unknown = set(document) - {"matrix", "variant", "exempt"}
    if unknown:
        raise MatrixError(f"unknown tables {sorted(unknown)}")
    table = dict(document.get("matrix") or {})
    allowed = {
        "package",
        "pin",
        "since",
        "resolved-before",
        "python",
        "probes",
        "scenarios",
        "profiles",
        "prereleases",
        "modes",
    }
    if extra := set(table) - allowed:
        raise MatrixError(f"unknown [matrix] keys {sorted(extra)}")
    try:
        package, since, resolved = (
            table["package"],
            table["since"],
            table["resolved-before"],
        )
        scenarios = tuple(table["scenarios"])
    except KeyError as missing:
        raise MatrixError(f"[matrix] needs {missing}") from None
    pin = table.get("pin", f"{package}=={{version}}")
    if "{version}" not in pin:
        raise MatrixError("[matrix] pin must contain {version}")
    probes = tuple(table.get("probes", scenarios[:1]))
    if not probes or (stray := [p for p in probes if p not in scenarios]):
        raise MatrixError("probes must be a non-empty subset of the matrix scenarios")
    profiles = {"default": {}, **table.get("profiles", {})}
    if profiles["default"]:
        raise MatrixError("the default profile sets no environment")
    modes = tuple(table.get("modes", ["native"]))
    if not set(modes) <= {"native", "sdk"}:
        raise MatrixError(f"modes must be native and/or sdk, not {modes}")
    variants = []
    for entry in document.get("variant", []):
        if extra := set(entry) - {
            "version",
            "profile",
            "also",
            "scenarios",
            "doc",
            "current",
        }:
            raise MatrixError(f"unknown [[variant]] keys {sorted(extra)}")
        profile = entry.get("profile", "default")
        if profile not in profiles:
            raise MatrixError(f"variant {entry.get('version')}: no profile {profile!r}")
        chosen = tuple(entry.get("scenarios", scenarios))
        if stray := [s for s in chosen if s not in scenarios]:
            raise MatrixError(
                f"variant {entry.get('version')}: {stray} not in the matrix scenarios"
            )
        if missing := [p for p in probes if p not in chosen]:
            raise MatrixError(
                f"variant {entry.get('version')}: must record the probes {missing}, which the census compares"
            )
        if not entry.get("doc", "").strip():
            raise MatrixError(
                f"variant {entry.get('version')}: say what it covers in `doc`"
            )
        variant = Variant(
            version=entry["version"],
            profile=profile,
            also=tuple(entry.get("also", ())),
            scenarios=chosen,
            doc=entry["doc"].strip(),
            current=bool(entry.get("current", False)),
        )
        if variant.current and variant.profile != "default":
            raise MatrixError(
                f"variant {variant.name}: the current release is captured without a profile"
            )
        if not _NAME.match(variant.name):
            raise MatrixError(
                f"variant name {variant.name!r} is not a portable directory name"
            )
        variants.append(variant)
    exemptions = {}
    for entry in document.get("exempt", []):
        if set(entry) != {"version", "reason", "revisit"}:
            raise MatrixError("[[exempt]] needs exactly version, reason and revisit")
        exemptions[entry["version"]] = (entry["reason"], entry["revisit"])
    if sum(v.current for v in variants) > 1:
        raise MatrixError("at most one variant is the current release")
    names = [v.name for v in variants]
    if len(set(names)) != len(names):
        raise MatrixError(f"repeated variants in {names}")
    return Matrix(
        suite=suite,
        package=package,
        pin=pin,
        since=since,
        resolved_before=resolved,
        python=str(table.get("python", "3.14")),
        probes=probes,
        scenarios=scenarios,
        profiles=profiles,
        prereleases=bool(table.get("prereleases", False)),
        modes=modes,
        variants=tuple(variants),
        exemptions=exemptions,
    )
