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
    #: Scenarios the census classifies but whose fixtures are not committed, each with the reason:
    #: a capture the parser cannot reconstruct at all would fail the golden invariants, and an
    #: expectation recording that failure must not be committed.
    withheld: dict[str, str] = field(default_factory=dict)

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
    pin: tuple[str, ...]
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
    #: Packages resolved as of each release's own release day rather than ``resolved_before``.
    era: tuple[str, ...]
    variants: tuple[Variant, ...] = field(default=())
    #: Releases the census could not run, each with the reviewed reason and the date it must be revisited.
    exemptions: dict[str, tuple[str, str]] = field(default_factory=dict)
    #: Releases whose model traffic differs from the suite's cassettes (another API, an extra call), each
    #: with the reason: they replay cassettes of their own, recorded live with ``--live``.
    recordings: dict[str, str] = field(default_factory=dict)
    #: ``python`` (a uv project under ``examples/python``) or ``javascript`` (a suite of the npm project).
    language: str = "python"
    #: Hosts a scenario may reach besides the local proxy and recorder, each declared in ``versions.toml``
    #: with its reason: replay is otherwise offline.
    allow_hosts: tuple[str, ...] = ()
    #: Seconds a scenario may run; a release that hangs instead of failing would otherwise hold the
    #: census for the default ten minutes per probe.
    timeout: float = 600

    @property
    def registry(self) -> str:
        return {"javascript": "npm", "go": "go", "java": "maven"}.get(
            self.language, "pypi"
        )

    def cassettes(self, version: str) -> Path:
        """The cassettes a release replays: its own live recording, or the suite's."""
        name = f"cassettes@{version}" if version in self.recordings else "cassettes"
        return self.suite / name

    def requirements(self, variant: Variant) -> list[str]:
        return [*self.pinned(variant.version), *variant.also]

    def pinned(self, version: str) -> list[str]:
        """The requirements installed for ``version``: the pin, and any companion packages it names."""
        return [p.format(version=version) for p in self.pin]

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
    unknown = set(document) - {"matrix", "variant", "exempt", "recording"}
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
        "era",
        "allow-hosts",
        "timeout",
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
    # One requirement, or several: a framework whose companion packages import its internals names them
    # bare, which lifts the suite's lower bound so `era` can resolve them as of the release's day.
    language = (
        suite.parent.name
        if suite.parent.name in ("javascript", "go", "java")
        else "python"
    )
    if language == "java" and "pin" not in table:
        # A JVM suite's versions are its Gradle catalog's, named by key, not by Maven coordinates.
        raise MatrixError(
            '[matrix] pin names the catalog version: pin = "<key>={version}"'
        )
    default_pin = (
        f"{package}@{{version}}"
        if language in ("javascript", "go")
        else f"{package}=={{version}}"
    )
    pins = table.get("pin", default_pin)
    pin = tuple([pins] if isinstance(pins, str) else pins)
    if not any("{version}" in p for p in pin):
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
            "withheld",
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
            withheld=dict(entry.get("withheld", {})),
        )
        if stray := [w for w in variant.withheld if w not in chosen]:
            raise MatrixError(
                f"variant {variant.name}: withholds {stray}, which it does not record"
            )
        if not all(str(reason).strip() for reason in variant.withheld.values()):
            raise MatrixError(
                f"variant {variant.name}: say why each scenario is withheld"
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
    recordings = {}
    for entry in document.get("recording", []):
        if set(entry) != {"version", "reason"} or not str(entry["reason"]).strip():
            raise MatrixError("[[recording]] needs exactly version and a reason")
        if entry["version"] in exemptions:
            raise MatrixError(f"{entry['version']} is both exempt and recorded")
        recordings[entry["version"]] = entry["reason"].strip()
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
        era=tuple(table.get("era", ())),
        variants=tuple(variants),
        exemptions=exemptions,
        recordings=recordings,
        language=language,
        allow_hosts=tuple(table.get("allow-hosts", ())),
        timeout=float(table.get("timeout", 600)),
    )
