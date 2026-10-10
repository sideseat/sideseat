"""A SideSeat server for the storage measurements: started over a work directory, loaded with exports, settled.

Imported by `storage-footprint.py` and `storage_gate_answers.py`. The server's own output goes to
`server.log` in its work directory, which a failed run keeps (`storage-footprint.py`), so a server that dies
leaves its reason behind.
"""

from __future__ import annotations

import collections
import json
import os
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path


def log(message: str) -> None:
    print(f"[storage] {message}", flush=True)


def http(method: str, url: str, body: bytes | None = None, headers: dict | None = None):
    request = urllib.request.Request(
        url, data=body, method=method, headers=headers or {}
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def log_tail(work: Path, lines: int = 40) -> str:
    """The end of the server's own output, or a note that there is none."""
    path = work / "server.log"
    if not path.exists():
        return "(no server.log)"
    return "\n".join(path.read_text(errors="replace").splitlines()[-lines:])


def start_server(
    binary: Path, work: Path, extra_env: dict, port: int
) -> subprocess.Popen:
    env = {
        "PATH": os.environ["PATH"],
        "HOME": str(work),
        "SIDESEAT_DATA_DIR": str(work),
        "SIDESEAT_SECRETS_BACKEND": "file",
        "SIDESEAT_PORT": str(port),
        "SIDESEAT_UI_PORT": str(port + 1),
        "SIDESEAT_OTEL_GRPC_PORT": str(port + 2),
        "SIDESEAT_RATE_LIMIT_ENABLED": "false",
        # The derived metric load alone is over a gigabyte of logical bytes in one project - above the default
        # 1 GiB project quota, which the server enforces and reclaims against. A measurement must store all it
        # sends, so the quota is set far above the load.
        "SIDESEAT_FILES_QUOTA_BYTES": str(1 << 40),
        **extra_env,
    }
    server = subprocess.Popen(
        [str(binary), "--no-auth"],
        cwd=work,
        env=env,
        stdout=open(work / "server.log", "wb"),
        stderr=subprocess.STDOUT,
    )
    for _ in range(90):
        try:
            if http("GET", f"http://127.0.0.1:{port}/api/v1/health")[0] == 200:
                return server
        except OSError:
            pass
        if server.poll() is not None:
            break
        time.sleep(1)
    sys.exit(f"[storage] server did not start:\n{log_tail(work)}")


def stop_server(server: subprocess.Popen) -> None:
    server.send_signal(signal.SIGTERM)
    try:
        server.wait(timeout=60)
    except subprocess.TimeoutExpired:
        server.kill()
        server.wait()


def alive(server: subprocess.Popen, work: Path, doing: str) -> None:
    """Fail, with the server's own last words, if it has exited."""
    if server.poll() is not None:
        sys.exit(
            f"[storage] the server exited with status {server.returncode} while {doing}:\n{log_tail(work)}"
        )


def load(
    exports: list[dict], port: int, server: subprocess.Popen, work: Path, root: Path
) -> dict[str, str]:
    """One project per tenant; every export posted once, retried while the server pushes back. Returns tenant ->
    project id, and reports every retry by the status that caused it: a retried export is one the server may have
    written already, which the stored figures must not count twice."""
    projects = {}
    base = f"http://127.0.0.1:{port}"
    for tenant in sorted({e["tenant"] for e in exports}):
        status, body = http(
            "POST",
            f"{base}/api/v1/projects",
            json.dumps({"name": tenant[:100], "organization_id": "default"}).encode(),
            {"Content-Type": "application/json"},
        )
        if status not in (200, 201):
            sys.exit(
                f"[storage] could not create project {tenant}: {status} {body[:200]!r}"
            )
        projects[tenant] = json.loads(body)["id"]
    started = time.monotonic()
    retries = collections.Counter()
    retried = 0
    for export in exports:
        content_type = (
            "application/json" if export["json"] else "application/x-protobuf"
        )
        url = f"{base}/otel/{projects[export['tenant']]}/v1/{export['signal']}"
        headers = {"Content-Type": content_type}
        try:
            status, body = http("POST", url, export["body"], headers)
            # 503 and 429 are back-pressure, not failure: the server is telling a collector to slow down, and a
            # collector retries. Treating them as errors made a long load fail on a full durability buffer.
            # Bounded by time, not attempts: how long a full buffer takes to drain depends on the disk - a Mac's
            # F_FULLFSYNC makes it seconds - and only a server that never drains is a failure.
            give_up = time.monotonic() + 600
            attempt = 0
            retried += status in (429, 503)
            while status in (429, 503) and time.monotonic() < give_up:
                retries[status] += 1
                attempt += 1
                time.sleep(min(0.05 * attempt, 1.0))
                status, body = http("POST", url, export["body"], headers)
        except OSError as error:
            alive(server, work, "loading")
            sys.exit(f"[storage] posting {export['path']} failed: {error}")
        if status != 200:
            # A derived load lives in the work directory, not in the repository, so the path is named as it is.
            path = export["path"]
            named = path.relative_to(root) if path.is_relative_to(root) else path
            sys.exit(f"[storage] {named} returned {status}: {body[:300]!r}")
    log(f"posted {len(exports)} exports in {time.monotonic() - started:.1f}s")
    log(
        f"retried {retried} exports, {sum(retries.values())} times: "
        + (
            ", ".join(
                f"{count} after a {status}" for status, count in sorted(retries.items())
            )
            or "none"
        )
    )
    return projects


def settle(
    projects: dict[str, str], port: int, server: subprocess.Popen, work: Path
) -> None:
    """Wait until every project's logical storage stops changing: ingestion is asynchronous past the ack."""
    base = f"http://127.0.0.1:{port}"
    previous, stable = None, 0
    for _ in range(180):
        alive(server, work, "settling")
        totals = []
        for project in projects.values():
            try:
                status, body = http("GET", f"{base}/api/v1/projects/{project}/storage")
            except OSError as error:
                alive(server, work, "settling")
                sys.exit(f"[storage] reading {project}'s storage failed: {error}")
            totals.append(
                json.loads(body).get("logical_bytes") if status == 200 else None
            )
        stable = stable + 1 if totals == previous else 0
        previous = totals
        if stable >= 5:
            return
        time.sleep(1)
    log("warning: storage accounting did not settle in 180s")
