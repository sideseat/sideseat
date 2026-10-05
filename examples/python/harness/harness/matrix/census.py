"""The census: every release of the support window, run once, classified by the telemetry shape it emits.

A variant is a release whose shape the fixtures hold; the census is the evidence that every *other*
release of the window emits one of those shapes too (or says why it could not be run). It reads the
package index, so it is the one matrix step that needs the network; replaying variants does not.
"""

from __future__ import annotations

import json
from collections import deque
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from functools import partial
from pathlib import Path
from typing import Any

from harness.capture import Suite
from harness.matrix import environment
from harness.matrix.run import replay
from harness.matrix.shape import digest, shape
from harness.matrix.spec import CENSUS, Matrix

FORMAT = "sideseat.version-census/1"


@dataclass(frozen=True)
class Release:
    version: str
    date: str


def releases(package: str, since: str, *, prereleases: bool) -> list[Release]:
    """Every non-yanked release of ``package`` uploaded on or after ``since``, oldest first."""
    from packaging.version import InvalidVersion, Version

    with urllib.request.urlopen(
        f"https://pypi.org/pypi/{package}/json", timeout=60
    ) as response:
        document = json.load(response)
    found = []
    for version, files in document["releases"].items():
        if not files or all(f.get("yanked") for f in files):
            continue
        try:
            parsed = Version(version)
        except InvalidVersion:
            continue
        if parsed.is_devrelease or (parsed.is_prerelease and not prereleases):
            continue
        date = min(f["upload_time_iso_8601"] for f in files)[:10]
        if date >= since:
            found.append((parsed, Release(version, date)))
    return [release for _, release in sorted(found, key=lambda pair: pair[0])]


def classify(
    suite: Suite, env_path: Path, matrix: Matrix, profile: str
) -> tuple[list[str], str | None]:
    """The shape lines of every probe under ``profile``, or the first probe's failure."""
    lines: list[str] = []
    for probe in matrix.probes:
        result = replay(suite, env_path, probe, env=matrix.profiles[profile])
        try:
            if not result.ok:
                return [], f"{probe}: " + "; ".join(result.problems)
            lines += [f"{probe} {line}" for line in shape(result.staging)]
        finally:
            result.discard()
    return sorted(lines), None


def run(
    matrix: Matrix, suite: Suite, *, jobs: int = 2, retry: bool = False, log: Any = None
) -> dict[str, Any]:
    """Classify every release of the window under every profile, and write the census.

    ``retry`` keeps the classified entries of the existing census and runs only the releases it has no
    shape for. Each environment is deleted once classified: a window holds dozens of releases, and their
    environments together are gigabytes.
    """
    log = log or partial(print, flush=True)
    window = releases(matrix.package, matrix.since, prereleases=matrix.prereleases)
    profiles = sorted(matrix.profiles)
    path = matrix.suite / CENSUS
    previous = (
        json.loads(path.read_text())
        if retry and path.exists()
        else {"releases": [], "shapes": {}}
    )
    kept = {(e["version"], e["profile"]): e for e in previous["releases"] if e["shape"]}
    shapes: dict[str, list[str]] = {
        e["shape"]: previous["shapes"][e["shape"]] for e in kept.values()
    }
    todo = [r for r in window if any((r.version, p) not in kept for p in profiles)]
    log(
        f"[census] {matrix.package}: {len(window)} releases since {matrix.since}, "
        f"{len(todo)} to run, profiles {profiles}, probes {list(matrix.probes)}"
    )

    def build(release: Release) -> tuple[Release, Path | None, str | None]:
        try:
            return (
                release,
                environment.ensure_requirements(
                    matrix,
                    f"{matrix.suite.name}/census/{release.version}",
                    [matrix.pin.format(version=release.version)],
                    released=release.date,
                ),
                None,
            )
        except environment.UnresolvableRelease as error:
            return release, None, f"does not resolve beside the harness: {error}"

    results: dict[tuple[str, str], dict[str, Any]] = dict(kept)
    with ThreadPoolExecutor(jobs) as pool:
        # Built at most ``jobs`` ahead and deleted once classified, so the window never holds more than
        # a few environments; classified one at a time, because the recorder is one per process.
        ahead = deque(pool.submit(build, r) for r in todo[:jobs])
        queued = iter(todo[jobs:])
        while ahead:
            release, env_path, failure = ahead.popleft().result()
            if (following := next(queued, None)) is not None:
                ahead.append(pool.submit(build, following))
            for profile in profiles:
                if (release.version, profile) in kept:
                    continue
                entry: dict[str, Any] = {
                    "version": release.version,
                    "date": release.date,
                    "profile": profile,
                    "shape": None,
                    "failure": failure,
                }
                if env_path is not None:
                    lines, entry["failure"] = classify(suite, env_path, matrix, profile)
                    if entry["failure"] is None:
                        entry["shape"] = digest(lines)
                        shapes[entry["shape"]] = lines
                log(
                    f"[census] {release.version} ({profile}): "
                    + (entry["shape"] or f"FAILED - {entry['failure'][:160]}")
                )
                results[(release.version, profile)] = entry
            if env_path is not None:
                environment.remove(env_path)
    order = {r.version: i for i, r in enumerate(window)}
    entries = sorted(
        (e for e in results.values() if e["version"] in order),
        key=lambda e: (order[e["version"]], e["profile"]),
    )
    used = {e["shape"] for e in entries if e["shape"]}
    document = {
        "format": FORMAT,
        "//": "Generated by `python -m harness matrix <producer> --census`; do not edit.",
        "package": matrix.package,
        "since": matrix.since,
        "resolved_before": matrix.resolved_before,
        "probes": list(matrix.probes),
        "releases": entries,
        "shapes": {k: v for k, v in sorted(shapes.items()) if k in used},
    }
    path.write_text(json.dumps(document, indent=1, ensure_ascii=False) + "\n")
    return document


def coverage(matrix: Matrix, document: dict[str, Any]) -> list[str]:
    """What the census says the variants leave uncovered; empty when every runnable release is held.

    A release (under a profile) is held when some variant emits the same shape - under any profile, since
    an opt-in a release does not know emits its default format. A variant is redundant when another holds
    its shape already, and aged out when its release has left the window.
    """
    problems = []
    by_release = {(e["version"], e["profile"]): e for e in document["releases"]}
    held: dict[str, str] = {}
    for variant in matrix.variants:
        entry = by_release.get((variant.version, variant.profile))
        if entry is None:
            problems.append(
                f"variant {variant.name}: its release is not in the census window"
            )
        elif entry["shape"] is None:
            problems.append(
                f"variant {variant.name}: the census could not run it ({entry['failure']})"
            )
        elif entry["shape"] in held:
            problems.append(
                f"variant {variant.name}: emits the shape {held[entry['shape']]} already holds; remove one"
            )
        else:
            held[entry["shape"]] = variant.name
    for entry in document["releases"]:
        if entry["shape"] is None and entry["version"] not in matrix.exemptions:
            problems.append(
                f"{entry['version']} ({entry['profile']}): not classified ({entry['failure']}); "
                "retake the census or exempt it with a reason"
            )
        if entry["shape"] is not None and entry["shape"] not in held:
            problems.append(
                f"{entry['version']} ({entry['profile']}): shape {entry['shape']} has no variant"
            )
    return problems
