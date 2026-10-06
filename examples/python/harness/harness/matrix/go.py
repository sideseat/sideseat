"""One copy of the Go module per variant of a Go suite, outside the repository.

The Go suites share one module (``examples/go``); a variant is a copy with the pinned modules moved by
``go get <module>@v<version>``. Go module versions are immutable and the module proxy serves every one, so
there is no resolution instant to hold; the rest of the module stays at the versions its ``go.mod`` pins.
"""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
from pathlib import Path

from harness.matrix.spec import Matrix


def project(matrix: Matrix) -> Path:
    return matrix.suite.parent


def split(requirement: str) -> tuple[str, str]:
    """``google.golang.org/adk@1.8.0`` as ``("google.golang.org/adk", "1.8.0")``."""
    module, _, version = requirement.rpartition("@")
    return module, version


def stamp(matrix: Matrix, requirements: list[str]) -> dict[str, object]:
    manifest = (project(matrix) / "go.mod").read_bytes()
    return {
        "requirements": requirements,
        "go.mod": hashlib.sha256(manifest).hexdigest(),
    }


def ensure(matrix: Matrix, environment: Path, requirements: list[str]) -> Path:
    """A copy of the Go module with ``requirements`` pinned, built if missing or stale."""
    from harness.matrix.environment import UnresolvableRelease

    wanted = stamp(matrix, requirements)
    marker = environment / "sideseat-matrix.json"
    if marker.exists() and json.loads(marker.read_text()) == wanted:
        return environment
    if environment.exists():
        shutil.rmtree(environment)
    shutil.copytree(project(matrix), environment)
    for requirement in requirements:
        module, version = split(requirement)
        result = subprocess.run(
            ["go", "get", f"{module}@v{version.removeprefix('v')}"],
            cwd=environment,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            shutil.rmtree(environment, ignore_errors=True)
            raise UnresolvableRelease(
                (result.stderr or result.stdout).strip().splitlines()[-12:]
            )
    # Compile once here, so a scenario's run measures the scenario and a build failure says so.
    result = subprocess.run(
        ["go", "build", "./..."], cwd=environment, capture_output=True, text=True
    )
    if result.returncode != 0:
        shutil.rmtree(environment, ignore_errors=True)
        raise UnresolvableRelease(
            (result.stderr or result.stdout).strip().splitlines()[-12:]
        )
    marker.write_text(json.dumps(wanted, indent=1) + "\n")
    return environment


def installed(environment: Path, *modules: str) -> dict[str, str]:
    """The versions the copy's go.mod requires, for the support matrix row."""
    found = {}
    for line in (environment / "go.mod").read_text().splitlines():
        parts = line.strip().split()
        if len(parts) >= 2 and parts[0] in modules:
            found[parts[0]] = parts[1].removeprefix("v")
        elif len(parts) >= 3 and parts[0] == "require" and parts[1] in modules:
            found[parts[1]] = parts[2].removeprefix("v")
    return found


def published(module: str) -> dict[str, str]:
    """Every version of ``module`` with its publication instant, as the go command reports them.

    Through the go command rather than the module proxy's HTTP API, so ``GOPROXY`` (``direct`` included)
    decides where versions come from, as it does for ``go get``.
    """
    listed = subprocess.run(
        ["go", "list", "-m", "-versions", module],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    found = {}
    for version in listed[1:]:
        info = subprocess.run(
            ["go", "list", "-m", "-json", f"{module}@{version}"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        found[version.removeprefix("v")] = json.loads(info)["Time"]
    return found
