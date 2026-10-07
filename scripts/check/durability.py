#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Prove that an OTLP success is sent only after everything it stands for is durable.

`make test-durability` loads `scripts/tools/synctrace` into a real server (DYLD_INSERT_LIBRARIES on macOS,
LD_PRELOAD on Linux), posts one captured trace export on each acknowledgement path, and reads back every
write, sync, rename and directory change the server made under its data directory between the request and
its 200. The acknowledgement is durable only if, before the 200:

- every file written was synced after its last write - with F_FULLFSYNC on macOS, where fsync stops at the
  drive's write cache;
- every new directory entry (a created file, a renamed file, a new directory) was followed by a sync of the
  directory that holds it.

The paths are persist-before-ack (the embedded default) and the durable queue (Redis in a container, with
`appendfsync always`). Each must show writes from the stores it uses, so a check that saw nothing fails
instead of passing.

    uv run --locked --script scripts/check/durability.py [--binary PATH] [--path default|queue]...
"""

from __future__ import annotations

import argparse
import collections
import os
import platform
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / "scripts/tools/synctrace"
FIXTURE = ROOT / "server/tests/fixtures/messages/vertex-ai/native/chat/req-001.pb"
APPLE = platform.system() == "Darwin"
REDIS_IMAGE = "redis:7"

# Files whose bytes are not meant to survive a crash: SQLite's shared-memory index is rebuilt from the WAL.
NOT_DURABLE_SUFFIXES = ("-shm",)

# Known and accepted: SQLite syncs the directory entry of a file it creates with a plain fsync whatever
# `PRAGMA fullfsync` says (its unix VFS passes no full-sync flag for directories). Reported, not failed.
RESIDUAL_PLAIN_DIRECTORY_SYNC = ("sqlite",)

# The stores each path must touch before its acknowledgement, by data-directory component.
EXPECTED = {
    "default": ("sqlite", "files", "duckdb"),
    "queue": ("sqlite", "files"),
}


def log(message: str) -> None:
    print(f"[durability] {message}", flush=True)


def build_interposer() -> Path:
    """Compile the interposer for this platform, once per source change."""
    out = TOOL / "build" / ("synctrace.dylib" if APPLE else "synctrace.so")
    source = TOOL / "synctrace.c"
    if out.exists() and out.stat().st_mtime >= source.stat().st_mtime:
        return out
    out.parent.mkdir(exist_ok=True)
    # The system compiler: on macOS a Homebrew clang in PATH can hang linking with its own LTO plugin.
    compiler = os.environ.get("CC") or ("/usr/bin/clang" if APPLE else "cc")
    flags = ["-dynamiclib"] if APPLE else ["-shared", "-fPIC", "-ldl"]
    subprocess.run(
        [
            compiler,
            *flags,
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-o",
            str(out),
            str(source),
        ],
        check=True,
        timeout=120,
    )
    return out


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def start_redis() -> tuple[str, int]:
    port = free_port()
    name = f"sideseat-durability-redis-{os.getpid()}"
    subprocess.run(
        [
            "docker",
            "run",
            "-d",
            "--rm",
            "--name",
            name,
            "-p",
            f"127.0.0.1:{port}:6379",
            REDIS_IMAGE,
            "redis-server",
            "--appendonly",
            "yes",
            "--appendfsync",
            "always",
        ],
        check=True,
        capture_output=True,
        timeout=300,
    )
    for _ in range(60):
        probe = subprocess.run(
            ["docker", "exec", name, "redis-cli", "ping"],
            capture_output=True,
            text=True,
        )
        if probe.stdout.strip() == "PONG":
            return name, port
        time.sleep(0.5)
    sys.exit("[durability] redis did not start")


def run_path(
    binary: Path, interposer: Path, path: str, extra_env: dict
) -> list[tuple[float, str, list[str]]]:
    """Run the server, post one export, and return the events between the request and its acknowledgement."""
    scratch = Path(tempfile.mkdtemp(prefix="sideseat-durability-"))
    # Resolved, because the interposer compares resolved paths and /var is /private/var on macOS.
    data = (scratch / "data").resolve()
    data.mkdir()
    trace = scratch / "trace.log"
    port = free_port()
    preload = "DYLD_INSERT_LIBRARIES" if APPLE else "LD_PRELOAD"
    env = {
        "PATH": os.environ["PATH"],
        "HOME": str(scratch),
        "SIDESEAT_DATA_DIR": str(data),
        "SIDESEAT_SECRETS_BACKEND": "file",
        "SIDESEAT_PORT": str(port),
        "SIDESEAT_UI_PORT": str(free_port()),
        "SIDESEAT_OTEL_GRPC_PORT": str(free_port()),
        "SIDESEAT_RATE_LIMIT_ENABLED": "false",
        "SYNCTRACE_ROOT": str(data),
        "SYNCTRACE_OUT": str(trace),
        preload: str(interposer),
        **extra_env,
    }
    server = subprocess.Popen(
        [str(binary), "--no-auth"],
        cwd=scratch,
        env=env,
        stdout=open(scratch / "server.log", "wb"),
        stderr=subprocess.STDOUT,
    )
    try:
        for _ in range(120):
            try:
                if (
                    urllib.request.urlopen(
                        f"http://127.0.0.1:{port}/api/v1/health", timeout=2
                    ).status
                    == 200
                ):
                    break
            except OSError:
                if server.poll() is not None:
                    sys.exit(
                        f"[durability] server exited:\n{(scratch / 'server.log').read_text()[-3000:]}"
                    )
                time.sleep(0.5)
        else:
            sys.exit("[durability] server did not become healthy")
        time.sleep(
            2
        )  # startup's own writes and checkpoints settle before the window opens
        sent = time.time()
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/otel/default/v1/traces",
            data=FIXTURE.read_bytes(),
            headers={"Content-Type": "application/x-protobuf"},
            method="POST",
        )
        status = urllib.request.urlopen(request, timeout=60).status
        acked = time.time()
        if status != 200:
            sys.exit(f"[durability] {path}: the export was answered {status}")
    finally:
        server.terminate()
        try:
            server.wait(timeout=30)
        except subprocess.TimeoutExpired:
            server.kill()
    events = []
    for line in trace.read_text().splitlines() if trace.exists() else []:
        stamp, event, *paths = line.split(" ")
        if sent <= float(stamp) <= acked:
            events.append(
                (float(stamp), event, [p.replace(str(data), "<data>") for p in paths])
            )
    shutil.rmtree(scratch, ignore_errors=True)
    return events


def judge(
    path: str, events: list[tuple[float, str, list[str]]]
) -> tuple[list[str], list[str]]:
    """Failures and accepted residuals for one path's events."""
    durable = {"fullfsync"} if APPLE else {"fsync"}
    syncs = collections.defaultdict(list)  # path -> [(time, event)]
    for stamp, event, paths in events:
        if event in ("fsync", "fullfsync", "barrierfsync"):
            syncs[paths[0]].append((stamp, event))

    failures, residuals = [], []

    def synced_after(target: str, after: float, directory: bool) -> None:
        later = [(t, e) for t, e in syncs.get(target, []) if t >= after]
        if any(e in durable for _, e in later):
            return
        store = target.removeprefix("<data>/").split("/")[0]
        if (
            APPLE
            and directory
            and store in RESIDUAL_PLAIN_DIRECTORY_SYNC
            and any(e == "fsync" for _, e in later)
        ):
            residuals.append(
                f"{target}: directory synced with plain fsync (SQLite's own directory sync)"
            )
            return
        seen = ", ".join(sorted({e for _, e in later})) or "nothing"
        kind = "directory" if directory else "file"
        failures.append(
            f"{target}: {kind} not made durable before the 200 (after the change: {seen})"
        )

    last_write = {}
    for stamp, event, paths in events:
        if event == "write" and not paths[0].endswith(NOT_DURABLE_SUFFIXES):
            last_write[paths[0]] = stamp
    for target, stamp in last_write.items():
        synced_after(target, stamp, directory=False)
    for stamp, event, paths in events:
        if event == "rename":
            synced_after(os.path.dirname(paths[1]), stamp, directory=True)
        elif event in ("create", "mkdir") and not paths[0].endswith(
            NOT_DURABLE_SUFFIXES
        ):
            synced_after(os.path.dirname(paths[0]), stamp, directory=True)

    failures = list(dict.fromkeys(failures))
    residuals = list(dict.fromkeys(residuals))
    touched = {p.removeprefix("<data>/").split("/")[0] for p in last_write}
    for store in EXPECTED[path]:
        if store not in touched:
            failures.append(
                f"no write to {store} before the 200 - the check saw nothing it could judge"
            )
    return failures, residuals


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--binary", type=Path, help="server binary (default: a fresh release build)"
    )
    parser.add_argument(
        "--path",
        action="append",
        choices=tuple(EXPECTED),
        help="ack path (default: all)",
    )
    args = parser.parse_args()
    paths = args.path or list(EXPECTED)

    binary = args.binary
    if binary is None:
        log("building release")
        subprocess.run(
            ["cargo", "build", "--locked", "--release", "-q", "-p", "sideseat-server"],
            cwd=ROOT,
            check=True,
        )
        target = subprocess.run(
            ["bash", str(ROOT / "scripts/dev/cargo-target-dir.sh")],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
        binary = Path(target) / "release/sideseat"
    interposer = build_interposer()

    failed = False
    for path in paths:
        redis = None
        extra = {}
        if path == "queue":
            redis = start_redis()
            extra = {
                "SIDESEAT_QUEUE_BACKEND": "redis",
                "SIDESEAT_CACHE_REDIS_URL": f"redis://127.0.0.1:{redis[1]}",
            }
        try:
            events = run_path(binary.resolve(), interposer, path, extra)
        finally:
            if redis:
                subprocess.run(["docker", "stop", redis[0]], capture_output=True)
        failures, residuals = judge(path, events)
        syncs = sum(
            1 for _, e, _ in events if e in ("fsync", "fullfsync", "barrierfsync")
        )
        log(f"{path}: {len(events)} events, {syncs} syncs before the 200")
        for residual in residuals:
            log(f"  residual: {residual}")
        for failure in failures:
            log(f"  FAIL: {failure}")
        failed |= bool(failures)
    log(
        "acknowledgements are durable"
        if not failed
        else "an acknowledgement is not durable"
    )
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
