"""``capture``: record the OTLP a scenario exports as a golden fixture.

::

    uv run --locked --directory examples/python/harness capture strands tool_use
    uv run --locked --directory examples/python/harness capture strands --mode sdk --model haiku
    uv run --locked --directory examples/python/harness capture strands --forward http://127.0.0.1:5388
    uv run --locked --directory examples/python/harness capture strands-js tool_use

A suite is a uv project under ``examples/python`` with a ``[tool.sideseat-example]`` table, or a
directory of ``examples/javascript`` with a ``suite.json``; both run the same ``sample`` command line.

The first mode of each scenario talks to Bedrock through a recording proxy (:mod:`harness.proxy`) and
saves the model's responses to ``<suite>/cassettes/<scenario>.json``; the other mode replays them, so
both runs hold the same conversation. ``--offline`` replays the committed cassettes in both modes.
A suite whose model is a ``fake-*`` alias needs neither: each run starts the deterministic fake
server in-process (:mod:`harness.fakes`), so captures are reproducible without credentials.

Each run also starts a telemetry recorder on a free local port, points the suite at it, and writes every trace
export to ``server/tests/fixtures/messages/<producer>/<mode>/<scenario>/req-NNN.*``, and every log
export beside them as ``logs-NNN.*`` - instrumentations that report the conversation as log events
linked to a span need both halves. The previous payloads of that scenario are replaced only when the
run succeeds, so a failed capture never leaves a half-written fixture behind. Metrics are acknowledged
and not recorded.

Capture needs the credentials the scenario's model needs. Regenerate the expectations afterwards and
read them before committing::

    UPDATE_GOLDENS=1 cargo nextest run --locked -p sideseat-server --test message_goldens
"""

from __future__ import annotations

import argparse
import getpass
import gzip
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import tomllib
import urllib.error
import urllib.request
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[4]
FIXTURES = REPO / "server" / "tests" / "fixtures" / "messages"
PYTHON_SUITES = REPO / "examples" / "python"
JAVASCRIPT_EXAMPLES = REPO / "examples" / "javascript"
#: The prompts, tools, scenarios and models of this harness, as the JavaScript harness reads them.
JAVASCRIPT_CONTENT = JAVASCRIPT_EXAMPLES / "harness" / "content.json"
PLACEHOLDER_USER = b"sideseat"


#: The Claude Code CLI stores a user's attachments under a directory named for its session, a fresh
#: UUID per run, and reports that path in place of the bytes.
_CLI_ATTACHMENT_DIR = re.compile(
    rb"(/claude-[^/]+/[^/]+/)[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}(/images/)"
)
_FIXED_SESSION = b"00000000-0000-0000-0000-000000000000"
#: The CLI names each subagent it starts with a fresh id and writes it into the text the model reads.
_CLI_AGENT_ID = re.compile(rb"agentId: (a[0-9a-f]{16})")
#: ...and reports how long the subagent took, a measurement no two runs share.
_CLI_SUBAGENT_DURATION = re.compile(rb"(duration_ms: )(\d+)")


def anonymise(raw: bytes, agents: dict[bytes, bytes] | None = None) -> bytes:
    """Replace what differs between two runs of one conversation, or names the person capturing it.

    The capturing user's account name, as it appears in file paths, becomes a placeholder, and the CLI's
    per-run attachment directory and subagent ids fixed ones - ``agents`` holds the run's ids, so one
    subagent keeps one id across payloads - so the native and SDK runs of a scenario compare. Every
    replacement has the same length: protobuf payloads are length-prefixed, so any other length would
    require re-encoding, and a re-encoded payload is no longer what the producer sent.
    """
    raw = _CLI_ATTACHMENT_DIR.sub(rb"\g<1>" + _FIXED_SESSION + rb"\g<2>", raw)
    raw = _CLI_SUBAGENT_DURATION.sub(lambda m: m[1] + b"0" * len(m[2]), raw)
    if agents is not None:
        for found in _CLI_AGENT_ID.findall(raw):
            agents.setdefault(found, b"a%016x" % (len(agents) + 1))
        for real, pinned in agents.items():
            raw = raw.replace(real, pinned)
    user = getpass.getuser().encode()
    if not user or user == PLACEHOLDER_USER:
        return raw
    return raw.replace(user, PLACEHOLDER_USER[: len(user)].ljust(len(user), b"_"))


#: The OTLP/HTTP paths that are recorded, and the file prefix each export is written under.
RECORDED_SIGNALS = {"/v1/traces": "req", "/v1/logs": "logs"}


def recorded_prefix(path: str) -> str | None:
    """The fixture prefix an export to ``path`` is recorded under, or ``None`` if it is not recorded."""
    for signal, prefix in RECORDED_SIGNALS.items():
        if signal in path:
            return prefix
    return None


class _Recorder(BaseHTTPRequestHandler):
    out: Path
    forward: str | None
    count = 0
    counts: dict[str, int] = {}
    agents: dict[bytes, bytes] = {}
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
        prefix = recorded_prefix(self.path)
        if prefix and body:
            raw = body
            if self.headers.get("Content-Encoding") == "gzip":
                raw = gzip.decompress(body)
            suffix = (
                "json"
                if self.headers.get("Content-Type", "").startswith("application/json")
                else "pb"
            )
            # One sequence per signal, so trace requests keep the numbering the golden runner replays in
            # whether or not the run also exported logs.
            with _Recorder.lock:
                number = _Recorder.counts.get(prefix, 0) + 1
                _Recorder.counts[prefix] = number
                if prefix == "req":
                    _Recorder.count = number
                path = self.out / f"{prefix}-{number:03d}.{suffix}"
                payload = anonymise(raw, _Recorder.agents)
            path.write_bytes(payload)
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


@dataclass(frozen=True)
class Suite:
    """A framework suite: a uv project under ``examples/python``, or a directory of the npm project
    under ``examples/javascript``. Both declare a producer, integrations and a default model."""

    root: Path
    language: str
    manifest: dict[str, Any]

    def sample(self, *args: str) -> list[str]:
        """The ``sample`` command, run in :attr:`root`."""
        if self.language == "javascript":
            # npm runs the package script above the suite and passes the suite as INIT_CWD.
            return ["npm", "run", "--silent", "sample", "--", *args]
        return ["uv", "run", "--locked", "sample", *args]


def suites() -> dict[str, Suite]:
    found = {}
    for manifest in sorted(PYTHON_SUITES.glob("*/pyproject.toml")):
        table = (
            tomllib.loads(manifest.read_text()).get("tool", {}).get("sideseat-example")
        )
        if table:
            found[table["producer"]] = Suite(manifest.parent, "python", table)
    for manifest in sorted(JAVASCRIPT_EXAMPLES.glob("*/suite.json")):
        table = json.loads(manifest.read_text())
        found[table["producer"]] = Suite(manifest.parent, "javascript", table)
    return found


def javascript_content() -> str:
    """``content.json``: what the JavaScript harness shares with this one, rendered from it.

    The JavaScript suites hold the same conversations as the Python ones, so their prompts, tool
    definitions, scenario catalog and model aliases are read from this document rather than written
    twice. ``tool_examples`` pins the tools' behaviour, which the JavaScript harness re-implements
    and checks against these results before any scenario runs.
    """
    from harness import catalog, content, models, tooling

    def outcome(function: Any, **arguments: Any) -> dict[str, Any]:
        try:
            return {"arguments": arguments, "result": function(**arguments)}
        except Exception as error:
            return {
                "arguments": arguments,
                "error": {"name": type(error).__name__, "message": str(error)},
            }

    tools = (content.get_weather, content.get_precipitation, content.book_flight)
    document = {
        "//": "Generated by examples/python/harness (capture --export-content); do not edit.",
        "prompts": {
            "system": content.SYSTEM,
            "chat": content.CHAT,
            "multi_turn": list(content.MULTI_TURN),
            "tool_use": content.TOOL_USE,
            "session": list(content.SESSION),
            "error": content.ERROR,
            "streaming": content.STREAMING,
            "structured": content.STRUCTURED,
            "reasoning": content.REASONING,
            "files": content.FILES,
            "multi_agent": content.MULTI_AGENT,
            "mcp": content.MCP,
        },
        "trip_plan": content.TripPlan.model_json_schema(),
        "tools": [
            {"name": s.name, "description": s.description, "parameters": s.parameters}
            for s in map(tooling.spec, tools)
        ],
        "tool_examples": {
            "get_weather": [
                outcome(content.get_weather, city=city, days=days)
                for city in ("Paris", "Tokyo", " barcelona ")
                for days in (0, 2, 9)
            ]
            + [outcome(content.get_weather, city="Rome")],
            "get_precipitation": [
                outcome(content.get_precipitation, city=city)
                for city in ("Paris", "Tokyo", "OSLO")
            ],
            "book_flight": [
                outcome(
                    content.book_flight,
                    origin="London",
                    destination="Oslo",
                    date="2026-11-14",
                )
            ],
        },
        "scenarios": [
            {"name": spec.name, "summary": spec.summary, "core": spec.core}
            for spec in catalog.CATALOG.values()
        ],
        "user_id": catalog.USER_ID,
        "models": {
            alias: {"surface": m.surface, "id": m.id, "reasoning": m.reasoning}
            for alias, m in models.MODELS.items()
        },
        "default_model": models.DEFAULT,
    }
    return json.dumps(document, indent=2, ensure_ascii=False) + "\n"


def export_javascript_content() -> None:
    rendered = javascript_content()
    if not JAVASCRIPT_CONTENT.exists() or JAVASCRIPT_CONTENT.read_text() != rendered:
        JAVASCRIPT_CONTENT.write_text(rendered)
        print(f"[capture] wrote {JAVASCRIPT_CONTENT.relative_to(REPO)}")


def scenarios_of(suite: Suite) -> list[str]:
    result = subprocess.run(
        suite.sample("--list"),
        cwd=suite.root,
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


def uses_fake_model(suite: Suite, model: str | None) -> bool:
    """Whether the suite runs a ``fake-*`` model: ``--model``, or the suite's default."""
    from harness import models

    alias = model or suite.manifest.get("default-model", models.DEFAULT)
    return models.resolve(alias).surface.startswith("fake-")


def capture_one(
    producer: str,
    suite: Suite,
    scenario: str,
    mode: str,
    model: str | None,
    forward: str | None,
    record: bool,
) -> bool:
    from harness.proxy import ModelProxy, client_environment

    cassette = suite.root / "cassettes" / f"{scenario}.json"
    # A fake model is deterministic and local: there is no traffic to record or replay.
    deterministic = uses_fake_model(suite, model)
    if not record and not deterministic and not cassette.exists():
        print(
            f"[capture] {producer}/{mode}/{scenario}: no cassette to replay at {cassette}"
        )
        return False
    staging = Path(tempfile.mkdtemp(prefix=f"capture-{producer}-{scenario}-"))
    _Recorder.out, _Recorder.forward, _Recorder.count = staging, forward, 0
    _Recorder.counts = {}
    _Recorder.agents = {}
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Recorder)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    arguments = [scenario]
    if mode == "sdk":
        arguments.append("--sideseat")
    if model:
        arguments += ["--model", model]
    command = suite.sample(*arguments)
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
    print(f"[capture] {producer}/{mode}/{scenario}: {' '.join(arguments)} ({action})")
    try:
        if deterministic:
            ok = (
                subprocess.run(command, cwd=suite.root, env=env, check=False).returncode
                == 0
            )
        else:
            with ModelProxy(cassette, record=record) as proxy:
                env.update(client_environment(proxy.url))
                ok = (
                    subprocess.run(
                        command, cwd=suite.root, env=env, check=False
                    ).returncode
                    == 0
                )
        if not deterministic and proxy.misses:
            print(
                f"[capture] the scenario made requests the cassette has no answer for: {proxy.misses}"
            )
            ok = False
    finally:
        server.shutdown()
    recorded = sorted(staging.glob("req-*"))
    log_exports = sorted(staging.glob("logs-*"))
    if not ok or not recorded:
        print(
            f"[capture] {producer}/{mode}/{scenario}: FAILED ({len(recorded)} request(s) recorded)"
        )
        shutil.rmtree(staging)
        return False
    target = FIXTURES / producer / mode / scenario
    target.mkdir(parents=True, exist_ok=True)
    for pattern in ("req-*", "logs-*"):
        for stale in target.glob(pattern):
            stale.unlink()
    for payload in [*recorded, *log_exports]:
        shutil.move(payload, target / payload.name)
    shutil.rmtree(staging)
    logged = f", {len(log_exports)} log export(s)" if log_exports else ""
    print(f"[capture] {producer}/{mode}/{scenario}: {len(recorded)} request(s){logged}")
    return True


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(
        prog="capture", description=__doc__.split("\n\n")[0]
    )
    parser.add_argument(
        "producer", nargs="?", help="suite producer, e.g. strands or strands-js"
    )
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
    parser.add_argument(
        "--export-content",
        action="store_true",
        help="write examples/javascript/harness/content.json and exit",
    )
    parser.add_argument(
        "--where",
        action="store_true",
        help="print the suite's language and directory and exit",
    )
    args = parser.parse_args(argv)

    if args.export_content:
        export_javascript_content()
        return
    known = suites()
    if args.producer not in known:
        raise SystemExit(f"no suite for {args.producer!r}; known: {', '.join(known)}")
    suite = known[args.producer]
    if args.where:
        print(suite.language, suite.root)
        return
    if suite.language == "javascript":
        # A JavaScript run reads what this harness says now, never a stale copy.
        export_javascript_content()
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
