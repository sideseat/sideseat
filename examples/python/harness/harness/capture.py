"""``capture``: record the OTLP a scenario exports as a golden fixture.

::

    uv run --locked --directory examples/python/harness capture strands tool_use
    uv run --locked --directory examples/python/harness capture strands --mode sdk --model haiku
    uv run --locked --directory examples/python/harness capture strands --forward http://127.0.0.1:5388

The first mode of each scenario talks to Bedrock through a recording proxy (:mod:`harness.proxy`) and
saves the model's responses to ``<suite>/cassettes/<scenario>.json``; the other mode replays them, so
both runs hold the same conversation. ``--offline`` replays the committed cassettes in both modes.
A suite whose model is a ``fake-*`` alias needs neither: each run starts the deterministic fake
server in-process (:mod:`harness.fakes`), so captures are reproducible without credentials.

Each run also starts a telemetry recorder on a free local port, points the suite at it, and writes every trace
export to ``server/tests/fixtures/messages/<producer>/<mode>/<scenario>/req-NNN.*``. The previous
payloads of that scenario are replaced only when the run succeeds, so a failed capture never leaves
a half-written fixture behind. Logs and metrics are acknowledged and not recorded.

Capture needs the credentials the scenario's model needs. Regenerate the expectations afterwards and
read them before committing::

    UPDATE_GOLDENS=1 cargo nextest run --locked -p sideseat-server --test message_goldens
"""

from __future__ import annotations

import argparse
import getpass
import gzip
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import tomllib
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]
FIXTURES = REPO / "server" / "tests" / "fixtures" / "messages"
PYTHON_SUITES = REPO / "examples" / "python"
PLACEHOLDER_USER = b"sideseat"


def anonymise(raw: bytes) -> bytes:
    """Replace the capturing user's account name, as it appears in file paths, with a placeholder.

    The replacement has the same length: protobuf payloads are length-prefixed, so any other length
    would require re-encoding, and a re-encoded payload is no longer what the producer sent.
    """
    user = getpass.getuser().encode()
    if not user or user == PLACEHOLDER_USER:
        return raw
    return raw.replace(user, PLACEHOLDER_USER[: len(user)].ljust(len(user), b"_"))


class _Recorder(BaseHTTPRequestHandler):
    out: Path
    forward: str | None
    count = 0
    lock = threading.Lock()

    def log_message(self, fmt: str, *args: object) -> None:
        pass

    def _body(self) -> bytes:
        # The OpenTelemetry JS exporter and the Claude Code CLI stream their exports chunked.
        if (self.headers.get("Transfer-Encoding") or "").lower() == "chunked":
            chunks = []
            while (line := self.rfile.readline().strip()) and (
                size := int(line.split(b";")[0], 16)
            ):
                chunks.append(self.rfile.read(size))
                self.rfile.readline()
            self.rfile.readline()
            return b"".join(chunks)
        length = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(length) if length else b""

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        body = self._body()
        if "/v1/traces" in self.path and body:
            raw = body
            if self.headers.get("Content-Encoding") == "gzip":
                raw = gzip.decompress(body)
            suffix = (
                "json"
                if self.headers.get("Content-Type", "").startswith("application/json")
                else "pb"
            )
            with _Recorder.lock:
                _Recorder.count += 1
                path = self.out / f"req-{_Recorder.count:03d}.{suffix}"
            path.write_bytes(anonymise(raw))
        status, reply = 200, b"{}"
        if self.forward:
            status, reply = self._forward(body)
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(reply)))
        self.end_headers()
        self.wfile.write(reply)

    def _forward(self, body: bytes) -> tuple[int, bytes]:
        assert self.forward is not None
        dropped = {"host", "content-length", "connection", "transfer-encoding"}
        headers = {k: v for k, v in self.headers.items() if k.lower() not in dropped}
        request = urllib.request.Request(
            self.forward.rstrip("/") + self.path,
            data=body,
            headers=headers,
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return response.status, response.read()
        except urllib.error.HTTPError as error:
            return error.code, error.read()
        except urllib.error.URLError as error:
            print(f"[capture] forward failed: {error}", file=sys.stderr)
            return 200, b"{}"


def python_suites() -> dict[str, Path]:
    suites = {}
    for manifest in sorted(PYTHON_SUITES.glob("*/pyproject.toml")):
        table = (
            tomllib.loads(manifest.read_text()).get("tool", {}).get("sideseat-example")
        )
        if table:
            suites[table["producer"]] = manifest.parent
    return suites


def scenarios_of(suite: Path) -> list[str]:
    result = subprocess.run(
        ["uv", "run", "--locked", "--directory", str(suite), "sample", "--list"],
        capture_output=True,
        text=True,
        check=True,
    )
    names = []
    for line in result.stdout.splitlines():
        if line.startswith("Models:"):
            break
        if line.startswith("  "):
            names.append(line.split()[0])
    return names


def uses_fake_model(suite: Path, model: str | None) -> bool:
    """Whether the suite runs a ``fake-*`` model: ``--model``, or the suite's default."""
    from harness import models

    table = tomllib.loads((suite / "pyproject.toml").read_text())["tool"][
        "sideseat-example"
    ]
    alias = model or table.get("default-model", models.DEFAULT)
    return models.resolve(alias).surface.startswith("fake-")


def capture_one(
    producer: str,
    suite: Path,
    scenario: str,
    mode: str,
    model: str | None,
    forward: str | None,
    record: bool,
) -> bool:
    from harness.proxy import ModelProxy, client_environment

    cassette = suite / "cassettes" / f"{scenario}.json"
    # A fake model is deterministic and local: there is no traffic to record or replay.
    deterministic = uses_fake_model(suite, model)
    if not record and not deterministic and not cassette.exists():
        print(
            f"[capture] {producer}/{mode}/{scenario}: no cassette to replay at {cassette}"
        )
        return False
    staging = Path(tempfile.mkdtemp(prefix=f"capture-{producer}-{scenario}-"))
    _Recorder.out, _Recorder.forward, _Recorder.count = staging, forward, 0
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Recorder)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    command = ["uv", "run", "--locked", "--directory", str(suite), "sample", scenario]
    if mode == "sdk":
        command.append("--sideseat")
    if model:
        command += ["--model", model]
    env = {
        **os.environ,
        "SIDESEAT_ENDPOINT": f"http://127.0.0.1:{server.server_address[1]}",
        "SIDESEAT_PROJECT_ID": "default",
    }
    env.pop("OTEL_EXPORTER_OTLP_ENDPOINT", None)
    action = (
        "fake model"
        if deterministic
        else "recording model traffic"
        if record
        else "replaying model traffic"
    )
    print(f"[capture] {producer}/{mode}/{scenario}: {' '.join(command[5:])} ({action})")
    try:
        if deterministic:
            ok = subprocess.run(command, env=env, check=False).returncode == 0
        else:
            with ModelProxy(cassette, record=record) as proxy:
                env.update(client_environment(proxy.url))
                ok = subprocess.run(command, env=env, check=False).returncode == 0
        if not deterministic and proxy.misses:
            print(
                f"[capture] the scenario made requests the cassette has no answer for: {proxy.misses}"
            )
            ok = False
    finally:
        server.shutdown()
    recorded = sorted(staging.glob("req-*"))
    if not ok or not recorded:
        print(
            f"[capture] {producer}/{mode}/{scenario}: FAILED ({len(recorded)} request(s) recorded)"
        )
        shutil.rmtree(staging)
        return False
    target = FIXTURES / producer / mode / scenario
    target.mkdir(parents=True, exist_ok=True)
    for stale in target.glob("req-*"):
        stale.unlink()
    for payload in recorded:
        shutil.move(payload, target / payload.name)
    shutil.rmtree(staging)
    print(f"[capture] {producer}/{mode}/{scenario}: {len(recorded)} request(s)")
    return True


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(
        prog="capture", description=__doc__.split("\n\n")[0]
    )
    parser.add_argument("producer", help="suite producer, e.g. strands")
    parser.add_argument(
        "scenario", nargs="*", help="scenarios to capture; default: all of them"
    )
    parser.add_argument("--mode", choices=("native", "sdk", "both"), default="both")
    parser.add_argument("--model", help="model alias passed to the suite")
    parser.add_argument(
        "--forward", help="also send every request to this SideSeat server"
    )
    parser.add_argument(
        "--offline",
        action="store_true",
        help="replay the committed model cassettes in both modes; needs no credentials",
    )
    args = parser.parse_args(argv)

    suites = python_suites()
    if args.producer not in suites:
        raise SystemExit(f"no suite for {args.producer!r}; known: {', '.join(suites)}")
    suite = suites[args.producer]
    scenarios = args.scenario or scenarios_of(suite)
    modes = ("native", "sdk") if args.mode == "both" else (args.mode,)
    failed = []
    for scenario in scenarios:
        for mode in modes:
            # The first live run of a scenario records the model traffic; every other run replays it,
            # so native and SDK telemetry describe the same conversation.
            record = not args.offline and mode == modes[0]
            if not capture_one(
                args.producer, suite, scenario, mode, args.model, args.forward, record
            ):
                failed.append(f"{args.producer}/{mode}/{scenario}")
    if failed:
        raise SystemExit(f"[capture] failed: {', '.join(failed)}")
    print(
        "[capture] next: UPDATE_GOLDENS=1 cargo nextest run --locked -p sideseat-server --test message_goldens"
    )
