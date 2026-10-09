"""``capture``: record the OTLP a scenario exports as a golden fixture.

::

    uv run --locked --directory examples/python/harness capture strands tool_use
    uv run --locked --directory examples/python/harness capture strands --mode sdk --model haiku
    uv run --locked --directory examples/python/harness capture strands --forward http://127.0.0.1:5388
    uv run --locked --directory examples/python/harness capture strands-js tool_use
    uv run --locked --directory examples/python/harness capture adk-go tool_use

A suite is a uv project under ``examples/python`` with a ``[tool.sideseat-example]`` table, or a
directory of ``examples/javascript``, ``examples/go`` or ``examples/java`` with a ``suite.json``; all
of them run the same ``sample`` command line.

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
and not recorded, except by ``--metrics``.

``--metrics`` captures the storage corpus's metric exports instead: the SDK's meter provider is switched on,
every metric export is written to ``server/tests/fixtures/metrics/<producer>/<mode>/<scenario>/metrics-NNN.*``,
and the run's traces and logs are discarded, so the message fixtures and their goldens are untouched. It
always replays the committed cassette and never records one, for the same reason.

Capture needs the credentials the scenario's model needs. Regenerate the expectations afterwards and
read them before committing::

    UPDATE_GOLDENS=1 cargo nextest run --locked -p sideseat-server --test message_goldens
"""

from __future__ import annotations

import argparse
import gzip
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import tomllib
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

from harness.scrub import scrub_account

REPO = Path(__file__).resolve().parents[4]
FIXTURES = REPO / "server" / "tests" / "fixtures" / "messages"
METRIC_FIXTURES = REPO / "server" / "tests" / "fixtures" / "metrics"
#: Seconds between metric exports in a ``--metrics`` capture. The SDK default is a minute, which a scenario
#: never reaches, so every run would record a single final export; a short interval records the cumulative
#: series a long-running service actually sends.
METRIC_EXPORT_INTERVAL_MS = "1000"
PYTHON_SUITES = REPO / "examples" / "python"
JAVASCRIPT_EXAMPLES = REPO / "examples" / "javascript"
GO_EXAMPLES = REPO / "examples" / "go"
JAVA_EXAMPLES = REPO / "examples" / "java"
#: The prompts, tools, scenarios and models of this harness, as the other languages' harnesses read
#: them: one rendered document, written beside each.
CONTENT_TARGETS = (
    JAVASCRIPT_EXAMPLES / "harness" / "content.json",
    GO_EXAMPLES / "harness" / "content.json",
    JAVA_EXAMPLES / "harness" / "content.json",
)
#: Where each language keeps its suites, all of them described by a ``suite.json``.
MANIFEST_SUITES = (
    ("javascript", JAVASCRIPT_EXAMPLES),
    ("go", GO_EXAMPLES),
    ("java", JAVA_EXAMPLES),
)


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
#: Browser Use names a tab after the last four hex digits of its CDP target id, fresh in every browser,
#: and writes that name into the page state the model reads and the actions it takes.
_BROWSER_TAB = re.compile(
    rb'(Tab |Current tab: |tab #|tab_id\\?"\s*:\s*\\?")([0-9A-F]{4})(?![0-9A-Za-z])'
)
#: Laminar writes every span's ancestry into an attribute as span ids in UUID form, and a rule reads the
#: last of them as the id of the tool call the span executed.
_LAMINAR_SPAN_ID = re.compile(rb"00000000-0000-0000-[0-9a-f]{4}-[0-9a-f]{12}")


#: Credential values a producer can serialise into telemetry: CrewAI writes its model client's config,
#: live keys included, into a span attribute. A name beside a structured value - Haystack's
#: `{"type": "env_var", ...}` reference - is not a credential.
_CREDENTIALS = re.compile(
    rb"(?:AKIA|ASIA)[0-9A-Z]{16}"
    rb'|(?:aws_secret_access_key|aws_session_token|secret_access_key|session_token)\\?"\s*:\s*\\?"[^"\\]{16,}'
)


def credential_in(payload: bytes) -> str | None:
    """The kind of credential a payload holds, if any - a fixture goes into git history for good."""
    found = _CREDENTIALS.search(payload)
    if found is None:
        return None
    return (
        "an AWS access key id"
        if found[0][:2] in (b"AK", b"AS")
        else "an AWS secret value"
    )


@dataclass
class Pins:
    """Names one run mints afresh, each pinned to a fixed one in order of first appearance.

    Held for a whole run, so a name keeps its pinned value across every payload of the run.
    """

    agents: dict[bytes, bytes] = field(default_factory=dict)
    tabs: dict[bytes, bytes] = field(default_factory=dict)
    spans: dict[bytes, bytes] = field(default_factory=dict)


def anonymise(raw: bytes, pins: Pins | None = None) -> bytes:
    """Replace what differs between two runs of one conversation, or names the person capturing it.

    The capturing user's account name, as it appears in file paths, becomes a placeholder; the CLI's
    per-run attachment directory and subagent ids, Browser Use's tab names and Laminar's span ids
    become fixed ones, so the native and SDK runs of a scenario compare. Every replacement has the
    same length: protobuf payloads are length-prefixed, so any other length would require re-encoding,
    and a re-encoded payload is no longer what the producer sent.
    """
    raw = _CLI_ATTACHMENT_DIR.sub(rb"\g<1>" + _FIXED_SESSION + rb"\g<2>", raw)
    raw = _CLI_SUBAGENT_DURATION.sub(lambda m: m[1] + b"0" * len(m[2]), raw)
    if pins is not None:
        agents, tabs, spans = pins.agents, pins.tabs, pins.spans
        for found in _CLI_AGENT_ID.findall(raw):
            agents.setdefault(found, b"a%016x" % (len(agents) + 1))
        for real, pinned in agents.items():
            raw = raw.replace(real, pinned)
        raw = _BROWSER_TAB.sub(
            lambda m: m[1] + tabs.setdefault(m[2], b"%04X" % (len(tabs) + 1)), raw
        )
        raw = _LAMINAR_SPAN_ID.sub(
            lambda m: m[0]
            if m[0] == _FIXED_SESSION
            else spans.setdefault(
                m[0], b"00000000-0000-0000-0000-%012x" % (len(spans) + 1)
            ),
            raw,
        )
    return scrub_account(raw)


#: The OTLP/HTTP paths that are recorded, and the file prefix each export is written under.
RECORDED_SIGNALS = {"/v1/traces": "req", "/v1/logs": "logs"}
#: What a ``--metrics`` run records in addition.
METRIC_SIGNALS = {**RECORDED_SIGNALS, "/v1/metrics": "metrics"}


def recorded_prefix(
    path: str, signals: dict[str, str] = RECORDED_SIGNALS
) -> str | None:
    """The fixture prefix an export to ``path`` is recorded under, or ``None`` if it is not recorded."""
    for signal, prefix in signals.items():
        if signal in path:
            return prefix
    return None


class _Recorder(BaseHTTPRequestHandler):
    out: Path
    forward: str | None
    signals: dict[str, str] = RECORDED_SIGNALS
    count = 0
    counts: dict[str, int] = {}
    pins = Pins()
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
        prefix = recorded_prefix(self.path, _Recorder.signals)
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
                payload = anonymise(raw, _Recorder.pins)
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
    """A framework suite: a uv project under ``examples/python``, or a ``suite.json`` directory of the
    npm, Go or Gradle project under ``examples/<language>``. Each declares a producer, integrations
    and a default model."""

    root: Path
    language: str
    manifest: dict[str, Any]

    def model_for(self, scenario: str, requested: str | None) -> str | None:
        """The model alias the suite runs ``scenario`` on, or ``None`` for its default."""
        from harness import models

        pinned = self.manifest.get("scenario-models") or {}
        return models.scenario_model(pinned, scenario, requested)

    def modes(self) -> tuple[str, ...]:
        """The telemetry modes this suite has a program for; every suite has `native` and `sdk`."""
        declared = self.manifest.get("modes") or ("native", "sdk")
        return tuple(declared)

    def sample(self, *args: str) -> list[str]:
        """The ``sample`` command, run in :attr:`root`."""
        if self.language == "javascript":
            # npm runs the package script above the suite and passes the suite as INIT_CWD.
            return ["npm", "run", "--silent", "sample", "--", *args]
        if self.language == "go":
            return ["go", "run", ".", *args]
        if self.language == "java":
            # The suite is a subproject of the Gradle build above it, run through that build's wrapper;
            # `run` takes one argument string.
            wrapper = "gradlew.bat" if os.name == "nt" else "gradlew"
            return [
                str(self.root.parent / wrapper),
                "-q",
                "--console=plain",
                "run",
                f"--args={shlex.join(args)}",
            ]
        return ["uv", "run", "--locked", "sample", *args]


def suites() -> dict[str, Suite]:
    found = {}
    for manifest in sorted(PYTHON_SUITES.glob("*/pyproject.toml")):
        table = (
            tomllib.loads(manifest.read_text()).get("tool", {}).get("sideseat-example")
        )
        if table:
            found[table["producer"]] = Suite(manifest.parent, "python", table)
    for language, examples in MANIFEST_SUITES:
        for manifest in sorted(examples.glob("*/suite.json")):
            table = json.loads(manifest.read_text())
            found[table["producer"]] = Suite(manifest.parent, language, table)
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


def export_content() -> None:
    rendered = javascript_content()
    for target in CONTENT_TARGETS:
        if not target.parent.is_dir():
            continue
        if not target.exists() or target.read_text() != rendered:
            target.write_text(rendered)
            print(f"[capture] wrote {target.relative_to(REPO)}")


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
    """Whether the suite runs a ``fake-*`` model: ``model``, or the suite's default."""
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
    metrics: bool = False,
    transcript_only: bool = False,
) -> bool:
    from harness import transcript
    from harness.proxy import ModelProxy, client_environment

    cassette = suite.root / "cassettes" / f"{scenario}.json"
    model = suite.model_for(scenario, model)
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
    _Recorder.signals = METRIC_SIGNALS if metrics else RECORDED_SIGNALS
    _Recorder.pins = Pins()
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Recorder)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    arguments = [scenario]
    if mode == "sdk":
        arguments.append("--sideseat")
    if mode == "logs":
        arguments.append("--logs")
    if model:
        arguments += ["--model", model]
    command = suite.sample(*arguments)
    env = {
        **os.environ,
        "SIDESEAT_ENDPOINT": f"http://127.0.0.1:{server.server_address[1]}",
        "SIDESEAT_PROJECT_ID": "default",
    }
    env.pop("OTEL_EXPORTER_OTLP_ENDPOINT", None)
    # Every process that serves the model - this one's proxy or fake, or a fake inside the suite - appends
    # the requests it receives here.
    request_log = staging / "model-requests.jsonl"
    env[transcript.ENV] = str(request_log)
    previous_log = os.environ.get(transcript.ENV)
    os.environ[transcript.ENV] = str(request_log)
    if metrics:
        env["SIDESEAT_CAPTURE_METRICS"] = "1"
        env["OTEL_METRIC_EXPORT_INTERVAL"] = METRIC_EXPORT_INTERVAL_MS
    action = (
        "fake model"
        if deterministic
        else "recording model traffic"
        if record
        else "replaying model traffic"
    )
    print(f"[capture] {producer}/{mode}/{scenario}: {' '.join(arguments)} ({action})")
    if deterministic and suite.language != "python":
        # A Python suite starts its fake in-process; a suite in another language is pointed at one.
        from harness import fakes
        from harness.clients import FAKE_PATHS
        from harness.models import resolve

        surface = resolve(model or suite.manifest.get("default-model", "")).surface
        env[f"{surface.upper().replace('-', '_')}_URL"] = (
            fakes.start(surface) + FAKE_PATHS[surface]
        )
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
        if previous_log is None:
            os.environ.pop(transcript.ENV, None)
        else:
            os.environ[transcript.ENV] = previous_log
    requests_document = (
        json.dumps(
            transcript.finish(request_log, lambda raw: anonymise(raw, _Recorder.pins)),
            indent=1,
        )
        + "\n"
    )
    recorded = sorted(staging.glob("req-*"))
    log_exports = sorted(staging.glob("logs-*"))
    metric_exports = sorted(staging.glob("metrics-*"))
    leaked = next(
        (
            (path.name, kind)
            for path in recorded + log_exports + metric_exports
            if (kind := credential_in(path.read_bytes()))
        ),
        None,
    ) or (
        (transcript.FILENAME, kind)
        if (kind := credential_in(requests_document.encode()))
        else None
    )
    if leaked:
        print(
            f"[capture] {producer}/{mode}/{scenario}: DISCARDED - {leaked[0]} holds {leaked[1]}; "
            "nothing was written to the fixtures"
        )
        shutil.rmtree(staging)
        return False
    for path in recorded + log_exports:
        if (size := path.stat().st_size) > 1_000_000:
            print(
                f"[capture] {producer}/{mode}/{scenario}: warning - {path.name} is {size // 1_000_000} MB; "
                "inlined media this large belongs in a fixture only by decision"
            )
    if not ok or not recorded:
        print(
            f"[capture] {producer}/{mode}/{scenario}: FAILED ({len(recorded)} request(s) recorded)"
        )
        shutil.rmtree(staging)
        return False
    if metrics:
        return _keep_metrics(producer, mode, scenario, staging, metric_exports)
    target = FIXTURES / producer / mode / scenario
    if transcript_only:
        if not any(target.glob("req-*")):
            # No committed fixture for this mode: there is no telemetry its requests would belong to.
            shutil.rmtree(staging)
            print(
                f"[capture] {producer}/{mode}/{scenario}: no fixture, nothing recorded"
            )
            return True
        (target / transcript.FILENAME).write_text(requests_document)
        # The committed telemetry stays: only what the framework sent the model is refreshed.
        shutil.rmtree(staging)
        print(f"[capture] {producer}/{mode}/{scenario}: model requests recorded")
        return True
    target.mkdir(parents=True, exist_ok=True)
    (target / transcript.FILENAME).write_text(requests_document)
    for pattern in ("req-*", "logs-*"):
        for stale in target.glob(pattern):
            stale.unlink()
    for payload in [*recorded, *log_exports]:
        shutil.move(payload, target / payload.name)
    shutil.rmtree(staging)
    logged = f", {len(log_exports)} log export(s)" if log_exports else ""
    print(f"[capture] {producer}/{mode}/{scenario}: {len(recorded)} request(s){logged}")
    return True


def _keep_metrics(
    producer: str, mode: str, scenario: str, staging: Path, exports: list[Path]
) -> bool:
    """Move a ``--metrics`` run's metric exports into the metric corpus; its traces and logs are dropped."""
    if not exports:
        print(
            f"[capture] {producer}/{mode}/{scenario}: FAILED (no metric export recorded)"
        )
        shutil.rmtree(staging)
        return False
    target = METRIC_FIXTURES / producer / mode / scenario
    target.mkdir(parents=True, exist_ok=True)
    for stale in target.glob("metrics-*"):
        stale.unlink()
    for payload in exports:
        shutil.move(payload, target / payload.name)
    shutil.rmtree(staging)
    print(f"[capture] {producer}/{mode}/{scenario}: {len(exports)} metric export(s)")
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
    # `both` is the pair every suite has; `logs` is captured on its own, since a suite that declares it has
    # a third program rather than a variation of the first two.
    parser.add_argument(
        "--mode", choices=("native", "sdk", "logs", "both"), default="both"
    )
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
        "--transcript-only",
        action="store_true",
        help="replay offline and refresh only each fixture's model-requests.json, keeping its telemetry",
    )
    parser.add_argument(
        "--metrics",
        action="store_true",
        help="record metric exports into the storage corpus instead of message fixtures",
    )
    parser.add_argument(
        "--export-content",
        action="store_true",
        help="write each language harness's content.json and exit",
    )
    parser.add_argument(
        "--where",
        action="store_true",
        help="print the suite's language and directory and exit",
    )
    args = parser.parse_args(argv)

    if args.export_content:
        export_content()
        return
    known = suites()
    if args.producer not in known:
        raise SystemExit(f"no suite for {args.producer!r}; known: {', '.join(known)}")
    suite = known[args.producer]
    if args.where:
        print(suite.language, suite.root)
        return
    if suite.language != "python":
        # Another language's run reads what this harness says now, never a stale copy.
        export_content()
    scenarios = args.scenario or scenarios_of(suite)
    modes = ("native", "sdk") if args.mode == "both" else (args.mode,)
    # A mode a suite has no program for would run its `native` program and be recorded under another
    # mode's name, which is a fixture describing a run that never happened.
    undeclared = [mode for mode in modes if mode not in suite.modes()]
    if undeclared:
        raise SystemExit(
            f"[capture] {args.producer} declares modes {list(suite.modes())}, not {undeclared}"
        )
    failed = []
    for scenario in scenarios:
        for mode in modes:
            # The first live run of a scenario records the model traffic; every other run replays it,
            # so native and SDK telemetry describe the same conversation.
            record = (
                not args.offline
                and not args.metrics
                and not args.transcript_only
                and mode == modes[0]
            )
            if not capture_one(
                args.producer,
                suite,
                scenario,
                mode,
                args.model,
                args.forward,
                record,
                metrics=args.metrics,
                transcript_only=args.transcript_only,
            ):
                failed.append(f"{args.producer}/{mode}/{scenario}")
    if failed:
        raise SystemExit(f"[capture] failed: {', '.join(failed)}")
    print(
        "[capture] next: UPDATE_GOLDENS=1 cargo nextest run --locked -p sideseat-server --test message_goldens"
    )
