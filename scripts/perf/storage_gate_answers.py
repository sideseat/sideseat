"""The storage gate's answers: the same reads over two stores, compared byte for byte.

Imported by `storage_gate_figures.py` (`--same-answers`). Two stores whose figures agree can still answer
differently - an engine's placement could reach a read's order - so the API reads behind `make bench-reads` and
the message views are asked of a server over each, and every status and body must match. Every read is chosen so
that it has an answer: each must be a 200 from both, so a read that fails alike cannot pass for one that agrees.
"""

from __future__ import annotations

import datetime
import shutil
import sqlite3
import subprocess
import tempfile
import time
import urllib.parse
from pathlib import Path

import storage_server


def log(message: str) -> None:
    print(f"[storage] {message}", flush=True)


def projects_of(store: Path) -> list[str]:
    database = sqlite3.connect(f"file:{store / 'sqlite/sideseat.db'}?mode=ro", uri=True)
    projects = [
        row[0] for row in database.execute("SELECT id FROM projects ORDER BY id")
    ]
    database.close()
    return projects


def iso(instant: datetime.datetime) -> str:
    return instant.strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def requests_for(store: Path) -> list[str]:
    """The reads asked of each project: its lists - whole and over its last 89 days of spans - the metric
    aggregates and a datapoint, the filter options, stats, the feed and a search for its commonest plain term that
    is no operator of the search language; and for its busiest trace and that trace's first span, their views and
    message views. Sessions are chosen by the server itself ([`session_requests`]): which session a trace
    belongs to is the store's own rule, over the revisions that win."""
    import duckdb

    analytics = duckdb.connect(str(store / "duckdb/sideseat.duckdb"), read_only=True)

    def first(sql: str, *values):
        row = analytics.execute(sql, list(values)).fetchone()
        return row if row and row[0] is not None else None

    requests = []
    for project in projects_of(store):
        base = f"/api/v1/project/{project}/otel"
        requests += [
            f"{base}/traces?limit=50&include_nongenai=true",
            f"{base}/spans?limit=50",
            f"{base}/sessions?limit=50",
            f"{base}/logs?limit=50",
            f"{base}/metrics?limit=50",
            f"{base}/metrics/aggregates?limit=50",
            f"{base}/feed/messages?limit=50",
            f"{base}/feed/spans?limit=50",
        ]
        requests += [
            f"{base}/{kind}/filter-options"
            for kind in ("traces", "spans", "sessions", "logs", "metrics")
        ]
        extent = first(
            "SELECT max(timestamp_start) FROM otel_spans WHERE project_id = ?", project
        )
        if extent:
            # The stats read refuses more than 90 days.
            until = extent[0] + datetime.timedelta(seconds=1)
            window = f"from_timestamp={iso(until - datetime.timedelta(days=89))}&to_timestamp={iso(until)}"
            requests += [
                f"{base}/stats?{window}",
                f"{base}/traces?limit=50&include_nongenai=true&{window}",
                f"{base}/spans?limit=50&{window}",
            ]
        datapoint = first(
            "SELECT datapoint_id FROM otel_metrics WHERE project_id = ? ORDER BY datapoint_id LIMIT 1",
            project,
        )
        if datapoint:
            requests.append(f"{base}/metrics/{datapoint[0]}")
        term = first(
            "SELECT term FROM span_terms WHERE project_id = ? AND regexp_matches(term, '^[a-z][a-z0-9]*$') AND term NOT IN ('and', 'or', 'not') "
            "GROUP BY term ORDER BY count(*) DESC, term LIMIT 1",
            project,
        )
        if term:
            requests.append(f"{base}/search?signal=spans&limit=50&q={term[0]}")
        trace = first(
            "SELECT trace_id FROM otel_spans WHERE project_id = ? GROUP BY trace_id "
            "ORDER BY count(*) DESC, trace_id LIMIT 1",
            project,
        )
        if trace:
            trace = trace[0]
            requests += [
                f"{base}/traces/{trace}",
                f"{base}/traces/{trace}/spans",
                f"{base}/traces/{trace}/messages",
                f"{base}/traces/{trace}/logs",
            ]
            span = first(
                "SELECT span_id FROM otel_spans WHERE project_id = ? AND trace_id = ? "
                "ORDER BY timestamp_start, span_id LIMIT 1",
                project,
                trace,
            )[0]
            requests += [
                f"{base}/traces/{trace}/spans/{span}",
                f"{base}/traces/{trace}/spans/{span}/messages",
                f"{base}/traces/{trace}/spans/{span}/logs",
            ]
    analytics.close()
    return requests


def binary_of(script, given: Path | None) -> Path:
    """The server to ask: the one named, or this checkout's, built."""
    if given:
        return given.resolve()
    target = subprocess.run(
        ["bash", str(script.ROOT / "scripts/dev/cargo-target-dir.sh")],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    log("building the server")
    subprocess.run(
        ["cargo", "build", "--locked", "-q", "-p", "sideseat-server"],
        cwd=script.ROOT,
        check=True,
    )
    return (Path(target) / "debug/sideseat").resolve()


def session_requests(get, projects: list[str]) -> list[str]:
    """The view and message view of each project's first listed session, as its server lists them."""
    import json

    requests = []
    for project in projects:
        base = f"/api/v1/project/{project}/otel"
        status, body = get(f"{base}/sessions?limit=50")
        listed = json.loads(body).get("data", []) if status == 200 else []
        if listed:
            quoted = urllib.parse.quote(listed[0]["session_id"], safe="")
            requests += [
                f"{base}/sessions/{quoted}",
                f"{base}/sessions/{quoted}/messages",
            ]
    return requests


def answers(store: Path, requests: list[str], script, binary: Path) -> dict:
    """Each request's status and body, from a server of its own over a copy of `store` - a server writes to its
    store - on ports no other server holds, checked to be serving this store's projects and still running."""
    with tempfile.TemporaryDirectory(dir=store.parent) as scratch:
        copy = Path(scratch) / "store"
        shutil.copytree(store, copy)
        port = storage_server.free_ports()
        server = storage_server.start_server(binary, copy, {}, port)
        try:
            time.sleep(0.5)
            if server.poll() is not None:
                raise SystemExit("[storage] the server for the answers exited")
            base = f"http://127.0.0.1:{port}"
            status, body = storage_server.http(
                "GET", f"{base}/api/v1/projects?limit=100"
            )
            for project in projects_of(store):
                if status != 200 or project.encode() not in body:
                    raise SystemExit(
                        f"[storage] the server on {port} does not serve {project}"
                    )

            def get(path: str):
                return storage_server.http("GET", base + path)

            asked = requests + session_requests(get, projects_of(store))
            return {path: get(path) for path in asked}
        finally:
            storage_server.stop_server(server)


def same_answers(ours: Path, theirs: Path, script, given: Path | None) -> int:
    """Fail unless the stores hold the same projects and a server over each answers every read with a 200 and
    the same bytes."""
    if projects_of(ours) != projects_of(theirs):
        log("FAIL: the stores hold different projects")
        return 1
    requests = sorted(set(requests_for(ours)) | set(requests_for(theirs)))
    binary = binary_of(script, given)
    mine = answers(ours, requests, script, binary)
    other = answers(theirs, requests, script, binary)
    requests = sorted(set(mine) | set(other))
    missing = (None, b"")
    differing = [
        path for path in requests if mine.get(path, missing) != other.get(path, missing)
    ]
    for path in differing[:20]:
        log(
            f"  {path}: {mine.get(path, missing)[0]} and {other.get(path, missing)[0]}, the bodies differing"
        )
    unanswered = [
        path
        for path in requests
        if mine.get(path, missing)[0] != 200 or other.get(path, missing)[0] != 200
    ]
    for path in unanswered[:20]:
        answer = mine.get(path, missing)
        log(
            f"  {path}: {answer[0]} and {other.get(path, missing)[0]}: {answer[1][:160]!r}"
        )
    if differing or unanswered:
        log(
            f"FAIL: of {len(requests)} reads, {len(differing)} answer differently and {len(unanswered)} "
            "do not answer 200"
        )
        return 1
    size = sum(len(body) for _, body in mine.values())
    log(
        f"the two stores answer all {len(requests)} reads alike, each a 200, {size:,} bytes"
    )
    return 0
