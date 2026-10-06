"""One uv environment per variant, outside the repository, rebuilt only when what it pins changes."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

from harness.matrix.spec import Matrix, Variant


def cache_root() -> Path:
    """``$SIDESEAT_MATRIX_CACHE``, else the platform's per-user cache directory."""
    if configured := os.environ.get("SIDESEAT_MATRIX_CACHE"):
        return Path(configured)
    if sys.platform == "win32":
        base = Path(os.environ.get("LOCALAPPDATA") or Path.home() / "AppData" / "Local")
    else:
        base = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache")
    return base / "sideseat" / "matrix"


def executable(environment: Path, name: str) -> Path:
    if sys.platform == "win32":
        return environment / "Scripts" / f"{name}.exe"
    return environment / "bin" / name


def stamp(
    matrix: Matrix, requirements: list[str], released: str | None = None
) -> dict[str, object]:
    """Everything an environment's contents depend on; a different stamp means a rebuild."""
    project = (matrix.suite / "pyproject.toml").read_bytes()
    return {
        "requirements": requirements,
        "era": {name: released for name in matrix.era} if released else {},
        "python": matrix.python,
        "resolved-before": matrix.resolved_before,
        "pyproject": hashlib.sha256(project).hexdigest(),
    }


def ensure(
    matrix: Matrix, variant: Variant, released: str | None, *, quiet: bool = True
) -> Path:
    """The variant's environment, built if missing or stale."""
    return ensure_requirements(
        matrix,
        f"{matrix.suite.name}/{variant.name}",
        matrix.requirements(variant),
        released=released,
        quiet=quiet,
    )


def ensure_requirements(
    matrix: Matrix,
    key: str,
    requirements: list[str],
    *,
    released: str | None = None,
    quiet: bool = True,
) -> Path:
    """An environment holding the suite's dependencies with ``requirements`` overriding theirs.

    The suite's own ``pyproject.toml`` is installed, so the harness and the SideSeat SDK come from their
    path sources exactly as the suite's lockfile has them; only the pinned packages move. Overrides
    rather than constraints, because a historical release may sit below the suite's lower bound.
    """
    environment = cache_root() / key
    if matrix.language == "javascript":
        from harness.matrix import npm

        return npm.ensure(matrix, environment, requirements, released=released)
    wanted = stamp(matrix, requirements, released)
    marker = environment / "sideseat-matrix.json"
    if marker.exists() and json.loads(marker.read_text()) == wanted:
        return environment
    if environment.exists():
        shutil.rmtree(environment)
    environment.parent.mkdir(parents=True, exist_ok=True)
    flags = ["-q"] if quiet else []
    subprocess.run(
        ["uv", "venv", *flags, "--python", matrix.python, str(environment)], check=True
    )
    common = ["--python", str(environment), "--exclude-newer", matrix.resolved_before]
    # The packages of the release's era resolve as of its release day: a release that left its
    # OpenTelemetry bound open was tested against the OpenTelemetry of its day, not today's.
    for name in matrix.era if released else ():
        common += ["--exclude-newer-package", f"{name}={released}T23:59:59Z"]
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as overrides:
        overrides.write("\n".join(requirements) + "\n")
    try:
        result = _uv(
            [
                "pip",
                "install",
                *flags,
                *common,
                "--overrides",
                overrides.name,
                "-r",
                str(matrix.suite / "pyproject.toml"),
                *requirements,
            ],
            matrix.suite,
        )
        if result.returncode != 0 and "No solution found" in result.stderr:
            result = _framework_first(matrix, requirements, flags, common)
    finally:
        os.unlink(overrides.name)
    if result.returncode != 0:
        shutil.rmtree(environment, ignore_errors=True)
        raise UnresolvableRelease(result.stderr.strip().splitlines()[-12:])
    marker.write_text(json.dumps(wanted, indent=1) + "\n")
    return environment


def _uv(arguments: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["uv", *arguments], cwd=cwd, capture_output=True, text=True)


def _framework_first(
    matrix: Matrix, requirements: list[str], flags: list[str], common: list[str]
) -> subprocess.CompletedProcess[str]:
    """Install the release with its own OpenTelemetry pins, then the harness without its dependencies.

    An older release can pin OpenTelemetry below the harness's lower bound (ADK 1.16 holds
    ``opentelemetry-sdk<=1.37``). The harness and SDK use only long-stable OpenTelemetry API, so the
    release decides; the harness's other dependencies are installed unversioned beside it.
    """
    suite_dependencies = _dependencies(matrix.suite / "pyproject.toml")
    harness_root = matrix.suite.parent / "harness"
    harness_dependencies = [
        d
        for d in _dependencies(
            harness_root / "pyproject.toml", extras=("bedrock", "openai", "anthropic")
        )
    ]
    own = [d for d in suite_dependencies if not _name(d).startswith("sideseat")]
    pinned = {_name(r) for r in requirements}
    framework = [d for d in own if _name(d) not in pinned] + requirements
    beside = sorted(
        {_name(d) for d in harness_dependencies if not _name(d).startswith("sideseat")}
    )
    first = _uv(["pip", "install", *flags, *common, *framework, *beside], matrix.suite)
    if first.returncode != 0:
        return first
    sdk = (matrix.suite.parent.parent.parent / "sdk" / "python").resolve()
    return _uv(
        [
            "pip",
            "install",
            *flags,
            *common,
            "--no-deps",
            "-e",
            str(harness_root),
            "-e",
            str(sdk),
        ],
        matrix.suite,
    )


def _name(requirement: str) -> str:
    return (
        re.split(r"[\[<>=!~ ;]", requirement, maxsplit=1)[0]
        .strip()
        .lower()
        .replace("_", "-")
    )


def _dependencies(project: Path, extras: tuple[str, ...] = ()) -> list[str]:
    document = tomllib.loads(project.read_text())["project"]
    found = list(document.get("dependencies", []))
    for extra in extras:
        found += document.get("optional-dependencies", {}).get(extra, [])
    return found


class UnresolvableRelease(RuntimeError):
    """The pinned release does not resolve beside the harness; the message is uv's explanation."""

    def __init__(self, lines: list[str]) -> None:
        super().__init__("\n".join(lines))


def remove(environment: Path) -> None:
    shutil.rmtree(environment, ignore_errors=True)


def installed(environment: Path, *packages: str) -> dict[str, str]:
    """The versions an environment actually holds, for the support matrix row."""
    if (environment / "package.json").exists():
        from harness.matrix import npm

        return npm.installed(environment, *packages)
    script = (
        "import importlib.metadata as m, json, sys\n"
        "out = {}\n"
        "for p in sys.argv[1:]:\n"
        "    try: out[p] = m.version(p)\n"
        "    except m.PackageNotFoundError: pass\n"
        "print(json.dumps(out))\n"
    )
    result = subprocess.run(
        [str(executable(environment, "python")), "-c", script, *packages],
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(result.stdout)
