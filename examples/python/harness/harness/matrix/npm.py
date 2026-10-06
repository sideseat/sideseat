"""One npm project per variant of a TypeScript suite, outside the repository.

The TypeScript suites share one npm project (``examples/javascript``), so a variant's environment is a copy
of that project with the pinned packages at their historical versions, installed as of ``resolved-before``
(``npm install --before``). The SideSeat SDK stays the repository's, linked by absolute path, exactly as the
project's own ``file:`` dependency has it.
"""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import sys
import urllib.parse
import urllib.request
from pathlib import Path

from harness.matrix.spec import Matrix

#: Never copied: installed packages, build output and the captures' own artefacts.
_SKIPPED = {"node_modules", "output", "cassettes", "tsconfig.tsbuildinfo"}


def project(matrix: Matrix) -> Path:
    """The npm project the suite belongs to: the directory above it."""
    return matrix.suite.parent


def split(requirement: str) -> tuple[str, str | None]:
    """``@scope/name@1.2.3`` as ``("@scope/name", "1.2.3")``; a bare name has no version."""
    head, at, version = requirement.rpartition("@")
    if not at or not head:
        return requirement, None
    return head, version


def stamp(
    matrix: Matrix, requirements: list[str], released: str | None
) -> dict[str, object]:
    manifest = (project(matrix) / "package.json").read_bytes()
    return {
        "requirements": requirements,
        "released": released,
        "resolved-before": matrix.resolved_before,
        "package.json": hashlib.sha256(manifest).hexdigest(),
    }


def ensure(
    matrix: Matrix,
    environment: Path,
    requirements: list[str],
    *,
    released: str | None = None,
) -> Path:
    """A copy of the suite's npm project with ``requirements`` pinned, built if missing or stale.

    The project is installed once as of ``resolved-before`` (the base); each variant clones the base and
    moves only its pinned packages, which takes seconds where a fresh install takes minutes.
    """
    from harness.matrix.environment import UnresolvableRelease, cache_root

    wanted = stamp(matrix, requirements, released)
    marker = environment / "sideseat-matrix.json"
    if marker.exists() and json.loads(marker.read_text()) == wanted:
        return environment
    if environment.exists():
        shutil.rmtree(environment)
    base = _base(matrix, cache_root() / matrix.suite.name / "npm-base")
    _clone(base, environment)
    before = matrix.resolved_before
    if released and matrix.era:
        # npm resolves as of one instant; a release's day is the closest it offers to resolving the
        # packages it moves as of that day.
        before = f"{released}T23:59:59Z"
    pins = [
        requirement if split(requirement)[1] is not None else f"{requirement}@*"
        for requirement in requirements
    ]
    result = _npm(["install", f"--before={before}", *pins], environment)
    if result.returncode != 0:
        shutil.rmtree(environment, ignore_errors=True)
        raise UnresolvableRelease(
            (result.stderr or result.stdout).strip().splitlines()[-12:]
        )
    marker.write_text(json.dumps(wanted, indent=1) + "\n")
    return environment


def _base(matrix: Matrix, base: Path) -> Path:
    """The suite's npm project installed as of ``resolved-before``, the SDK linked from the repository."""
    from harness.matrix.environment import UnresolvableRelease

    wanted = stamp(matrix, [], None)
    marker = base / "sideseat-matrix.json"
    if marker.exists() and json.loads(marker.read_text()) == wanted:
        return base
    if base.exists():
        shutil.rmtree(base)
    source = project(matrix)
    shutil.copytree(
        source, base, ignore=lambda _dir, names: [n for n in names if n in _SKIPPED]
    )
    manifest_path = base / "package.json"
    manifest = json.loads(manifest_path.read_text())
    for table in ("dependencies", "devDependencies"):
        for name, spec in list(manifest.get(table, {}).items()):
            # A relative file: dependency (the SideSeat SDK) points into the repository from the copy too.
            if isinstance(spec, str) and spec.startswith("file:"):
                target = (source / spec.removeprefix("file:")).resolve()
                manifest[table][name] = f"file:{target}"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    (base / "package-lock.json").unlink(missing_ok=True)
    result = _npm(["install", f"--before={matrix.resolved_before}"], base)
    if result.returncode != 0:
        shutil.rmtree(base, ignore_errors=True)
        raise UnresolvableRelease(
            (result.stderr or result.stdout).strip().splitlines()[-12:]
        )
    marker.write_text(json.dumps(wanted, indent=1) + "\n")
    return base


def _clone(source: Path, target: Path) -> None:
    """A copy of ``source``; copy-on-write where the file system offers it (APFS), so it costs no space."""
    target.parent.mkdir(parents=True, exist_ok=True)
    if sys.platform == "darwin":
        subprocess.run(["cp", "-cR", str(source), str(target)], check=True)
    else:
        shutil.copytree(source, target, symlinks=True)
    (target / "sideseat-matrix.json").unlink(missing_ok=True)


def _npm(arguments: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["npm", *arguments, "--no-audit", "--no-fund", "--loglevel=error"],
        cwd=cwd,
        capture_output=True,
        text=True,
    )


def installed(environment: Path, *packages: str) -> dict[str, str]:
    """The versions the copy actually holds, for the support matrix row."""
    found = {}
    for name in packages:
        manifest = environment / "node_modules" / name / "package.json"
        if manifest.exists():
            found[name] = json.loads(manifest.read_text())["version"]
    return found


def published(package: str) -> dict[str, str]:
    """Every published version of ``package`` with its upload instant, from the npm registry."""
    url = "https://registry.npmjs.org/" + urllib.parse.quote(package, safe="@")
    with urllib.request.urlopen(url, timeout=60) as response:
        document = json.load(response)
    return {
        version: instant
        for version, instant in document.get("time", {}).items()
        if version not in ("created", "modified")
        and version in document.get("versions", {})
    }
