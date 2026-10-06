"""One copy of the Gradle build per variant of a JVM suite, outside the repository.

The JVM suites share one Gradle build (``examples/java``) whose dependency versions live in its version
catalog (``gradle/libs.versions.toml``). A variant is a copy of the build with the pinned catalog versions
replaced: a pin is ``<catalog version key>={version}``. Maven artefacts are immutable, so, as for Go,
there is no resolution instant to hold.
"""

from __future__ import annotations

import email.utils
import hashlib
import json
import re
import shutil
import subprocess
import urllib.error
import urllib.request
import xml.etree.ElementTree as ElementTree
from pathlib import Path

from harness.matrix.spec import Matrix

CENTRAL = "https://repo1.maven.org/maven2"
CATALOG = Path("gradle") / "libs.versions.toml"
#: Every variant build and run: a census builds dozens of variants, and Gradle's defaults (a worker per core,
#: a Kotlin compile daemon, a build daemon per variant's copy) would saturate the machine it shares.
GRADLE_FLAGS = (
    "-q",
    "--console=plain",
    "--no-daemon",
    "--max-workers=2",
    "-Pkotlin.compiler.execution.strategy=in-process",
)
#: Never copied: build output, Gradle's and Kotlin's state, and the captures' own artefacts.
_SKIPPED = {"build", ".gradle", ".kotlin", "cassettes"}


def project(matrix: Matrix) -> Path:
    return matrix.suite.parent


def split(requirement: str) -> tuple[str, str]:
    """``adk=1.8.0`` as ``("adk", "1.8.0")``: a catalog version key and its version."""
    key, _, version = requirement.partition("=")
    return key.strip(), version.strip()


def stamp(matrix: Matrix, requirements: list[str]) -> dict[str, object]:
    catalog = (project(matrix) / CATALOG).read_bytes()
    return {
        "requirements": requirements,
        "catalog": hashlib.sha256(catalog).hexdigest(),
    }


def pin(catalog: str, key: str, version: str) -> str:
    """The catalog with ``[versions] key`` set to ``version``; a key the catalog lacks is an error."""
    pattern = re.compile(rf'^(\s*{re.escape(key)}\s*=\s*)"[^"]*"', re.MULTILINE)
    if not pattern.search(catalog):
        raise ValueError(f"the version catalog has no version {key!r}")
    return pattern.sub(lambda m: f'{m.group(1)}"{version}"', catalog, count=1)


def ensure(matrix: Matrix, environment: Path, requirements: list[str]) -> Path:
    """A copy of the Gradle build with ``requirements`` pinned and the suite compiled."""
    from harness.matrix.environment import UnresolvableRelease

    wanted = stamp(matrix, requirements)
    marker = environment / "sideseat-matrix.json"
    if marker.exists() and json.loads(marker.read_text()) == wanted:
        return environment
    if environment.exists():
        shutil.rmtree(environment)
    shutil.copytree(
        project(matrix),
        environment,
        ignore=lambda _dir, names: [n for n in names if n in _SKIPPED],
    )
    catalog = (environment / CATALOG).read_text()
    for requirement in requirements:
        catalog = pin(catalog, *split(requirement))
    (environment / CATALOG).write_text(catalog)
    result = subprocess.run(
        [str(environment / "gradlew"), *GRADLE_FLAGS, f":{matrix.suite.name}:classes"],
        cwd=environment,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        shutil.rmtree(environment, ignore_errors=True)
        raise UnresolvableRelease(
            (result.stderr or result.stdout).strip().splitlines()[-12:]
        )
    marker.write_text(json.dumps(wanted, indent=1) + "\n")
    return environment


def installed(environment: Path, *keys: str) -> dict[str, str]:
    """The catalog versions the copy pins, for the support matrix row."""
    catalog = (environment / CATALOG).read_text()
    found = {}
    for key in keys:
        if match := re.search(
            rf'^\s*{re.escape(key)}\s*=\s*"([^"]*)"', catalog, re.MULTILINE
        ):
            found[key] = match.group(1)
    return found


def published(coordinates: str) -> dict[str, str]:
    """Every version of ``group:artifact`` on Maven Central, with the instant its POM was published."""
    group, artifact = coordinates.split(":")
    base = f"{CENTRAL}/{group.replace('.', '/')}/{artifact}"
    with urllib.request.urlopen(f"{base}/maven-metadata.xml", timeout=60) as response:
        metadata = ElementTree.fromstring(response.read())
    found = {}
    for node in metadata.iterfind("./versioning/versions/version"):
        version = (node.text or "").strip()
        request = urllib.request.Request(
            f"{base}/{version}/{artifact}-{version}.pom", method="HEAD"
        )
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                modified = response.headers.get("Last-Modified")
        except urllib.error.HTTPError:
            continue
        if modified:
            found[version] = email.utils.parsedate_to_datetime(modified).isoformat()
    return found
