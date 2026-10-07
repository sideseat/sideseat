"""Capture a coding-agent CLI's OpenTelemetry as golden fixtures: Claude Code and the Codex CLI.

::

    # record: the model traffic goes to Bedrock through the harness proxy and is saved as a cassette
    set -a; . ~/.aws/sideseat-capture.env; set +a
    uv run --locked --project examples/python/harness python examples/cli/capture.py claude-code
    uv run --locked --project examples/python/harness python examples/cli/capture.py codex tool_use

    # replay the committed cassettes: no credentials, no model calls
    uv run --locked --project examples/python/harness python examples/cli/capture.py codex --offline

A CLI is not a library a suite can import, so this drives the installed binary instead: one process per
user turn, a later turn resuming the session the first one started. Everything else is the Python
harness's - the telemetry recorder and its anonymisation, the credential check, the recording model
proxy, and the scenario content - so a CLI fixture is recorded exactly as a framework fixture is.

Each run is isolated from the machine it runs on: a fresh ``HOME`` (and ``CODEX_HOME``), a fixed
workspace under ``/tmp/sideseat-cli``, no inherited environment, and no credentials at all - the proxy
signs for ``bedrock-runtime`` itself, so the CLI never sees an AWS key. Requests that are not model calls
(Codex posts turn-cost analytics to its provider's base URL) are answered locally and not forwarded.

The binaries default to ``claude`` and ``codex`` on ``PATH``; ``CLAUDE_BIN`` and ``CODEX_BIN`` name others.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import uuid
from dataclasses import dataclass
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import Any

from harness import capture, content, transcript
from harness.proxy import ModelProxy

HERE = Path(__file__).resolve().parent
#: Fixed rather than ``tempfile.gettempdir()``: macOS gives every account its own random temp directory,
#: and the CLIs write the workspace path into the telemetry and into what the model reads.
SCRATCH = Path("/tmp/sideseat-cli")
MODEL_PATHS = re.compile(r"^/model/|^/openai/v1/(responses|chat/completions)")
SONNET = "global.anthropic.claude-sonnet-5-5"
GPT = "global.openai.gpt-6.1-sol"


def forecast() -> str:
    """The file the agent reads: the shared weather tool's answers, so the facts match every suite's."""
    cities = {city: content.get_weather(city, days=2) for city in ("Paris", "Tokyo")}
    return json.dumps(cities, indent=2) + "\n"


TOOL_USE = (
    "Read forecast.json in this directory, then run the shell command `wc -l forecast.json`. "
    "Using what the file says, tell me in two sentences how many lines it has and whether I should "
    "pack an umbrella for Paris or for Tokyo."
)
ERROR = (
    "Run the shell command `cat bookings/london-oslo.txt` to read my flight booking, then tell me in "
    "one sentence what happened."
)
MULTI_AGENT = (
    "Delegate to a sub-agent: have it read forecast.json and report the Tokyo forecast. Then, using its "
    "report, write a one-sentence packing tip for Tokyo."
)


@dataclass(frozen=True)
class Scenario:
    name: str
    turns: tuple[str, ...]
    #: Whether the agent may start a sub-agent.
    delegates: bool = False


SCENARIOS = {
    s.name: s
    for s in (
        Scenario("tool_use", (TOOL_USE,)),
        Scenario("multi_turn", content.MULTI_TURN),
        Scenario("error", (ERROR,)),
        Scenario("multi_agent", (MULTI_AGENT,), delegates=True),
    )
}


class CliProxy(ModelProxy):
    """The harness proxy, forwarding model calls only."""

    def _respond(
        self, method: str, path: str, body: bytes, headers: dict[str, str]
    ) -> dict[str, Any]:
        if not MODEL_PATHS.match(path):
            return {
                "status": 200,
                "headers": {"content-type": "application/json"},
                "body": "e30=",
            }
        return super()._respond(method, path, body, headers)


@dataclass(frozen=True)
class Run:
    scenario: Scenario
    mode: str
    workspace: Path
    home: Path
    telemetry: str
    proxy: str


class Cli:
    producer: str
    binary_env: str
    binary_name: str
    modes: tuple[str, ...]
    #: What the model said that this CLI never exports, by truth fact kind, with the reason: the truth
    #: derived from a cassette asks for it otherwise, and no reading could ever supply it.
    #: ``text`` means the assistant's text in any response but a turn's final one.
    unexported: dict[str, str] = {}

    def binary(self) -> str:
        found = os.getenv(self.binary_env) or shutil.which(self.binary_name)
        if not found:
            raise SystemExit(f"{self.binary_name} not found; set {self.binary_env}")
        return found

    def environment(self, run: Run) -> dict[str, str]:
        path = os.pathsep.join(
            [str(Path(self.binary()).parent), "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        )
        return {
            "PATH": path,
            "HOME": str(run.home),
            "TMPDIR": "/tmp",
            "LANG": "en_US.UTF-8",
        }

    def turn(self, run: Run, index: int, prompt: str) -> list[str]:
        raise NotImplementedError

    def prepare(self, run: Run) -> None:
        pass


class ClaudeCode(Cli):
    """Claude Code on Bedrock, in ``-p`` mode.

    ``native`` is the fullest documented setup: the enhanced (beta) tracing tier with its detailed
    content attributes, plus log events and metrics. ``logs`` leaves out the detailed tier, which needs
    org allowlisting for interactive sessions: there the conversation's text reaches telemetry only as
    log events and the tools' ``tool.output`` span events.
    """

    producer = "claude-code"
    binary_env = "CLAUDE_BIN"
    binary_name = "claude"
    modes = ("native", "logs")

    def environment(self, run: Run) -> dict[str, str]:
        env = super().environment(run)
        env.update(
            {
                "CLAUDE_CODE_USE_BEDROCK": "1",
                "AWS_REGION": "us-east-1",
                "ANTHROPIC_BEDROCK_BASE_URL": run.proxy,
                "CLAUDE_CODE_SKIP_BEDROCK_AUTH": "1",
                "ANTHROPIC_MODEL": SONNET,
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": SONNET,
                "ANTHROPIC_SMALL_FAST_MODEL": SONNET,
                # Without these the CLI adds memory files, CLAUDE.md instructions and git guidance.
                "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1",
                "CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1",
                "CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS": "1",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
                "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
                "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA": "1",
                "OTEL_TRACES_EXPORTER": "otlp",
                "OTEL_LOGS_EXPORTER": "otlp",
                "OTEL_METRICS_EXPORTER": "otlp",
                "OTEL_EXPORTER_OTLP_PROTOCOL": "http/protobuf",
                "OTEL_EXPORTER_OTLP_ENDPOINT": run.telemetry,
                "OTEL_LOG_USER_PROMPTS": "1",
                "OTEL_LOG_TOOL_DETAILS": "1",
                "OTEL_TRACES_EXPORT_INTERVAL": "1000",
                "OTEL_LOGS_EXPORT_INTERVAL": "1000",
                "OTEL_METRIC_EXPORT_INTERVAL": "1000",
            }
        )
        if run.mode == "native":
            env["ENABLE_BETA_TRACING_DETAILED"] = "1"
            env["BETA_TRACING_ENDPOINT"] = run.telemetry
        else:
            # The only place a tool's output is exported without the detailed tier. Beside that tier it
            # is a second copy of each result, and one that names no call, so it is left off there.
            env["OTEL_LOG_TOOL_CONTENT"] = "1"
        return env

    def turn(self, run: Run, index: int, prompt: str) -> list[str]:
        tools = "Read,Bash,Agent" if run.scenario.delegates else "Read,Bash"
        # A fixed session id per scenario, so recaptures name the same session.
        session = str(
            uuid.uuid5(
                uuid.NAMESPACE_URL, f"sideseat:{self.producer}:{run.scenario.name}"
            )
        )
        resume = ["--resume", session] if index else ["--session-id", session]
        return [
            self.binary(),
            "-p",
            prompt,
            *resume,
            "--model",
            SONNET,
            "--setting-sources",
            "",
            "--tools",
            tools,
            "--allowedTools",
            tools,
            "--output-format",
            "json",
        ]


class Codex(Cli):
    """The Codex CLI in ``exec`` mode, with every ``[otel]`` exporter and both content opt-ins on.

    Codex ties a log event to a span only while its trace exporter runs; with logs alone every record
    is unattached, so the trace exporter is part of the setup rather than an extra.
    """

    producer = "codex"
    binary_env = "CODEX_BIN"
    binary_name = "codex"
    modes = ("native",)
    unexported = {
        "system": "Codex exports no system prompt or instructions, in any signal",
        "text": "Codex exports a turn's final reply (codex.agent_response) and nothing the model wrote "
        "between tool calls",
    }

    def environment(self, run: Run) -> dict[str, str]:
        return {**super().environment(run), "CODEX_HOME": str(run.home / ".codex")}

    def prepare(self, run: Run) -> None:
        home = run.home / ".codex"
        home.mkdir(parents=True)
        endpoint = run.telemetry
        (home / "config.toml").write_text(
            f"""model = "{GPT}"
model_provider = "bedrock-proxy"
check_for_update_on_startup = false
approval_policy = "never"
sandbox_mode = "workspace-write"
# Bedrock's Responses endpoint rejects the hosted web search tool.
web_search = "disabled"

[model_providers.bedrock-proxy]
# Not "Amazon Bedrock": Codex treats a provider of that name as its built-in one and signs requests itself.
name = "Bedrock capture proxy"
base_url = "{run.proxy}/openai/v1"
wire_api = "responses"

[analytics]
enabled = false

[feedback]
enabled = false

[otel]
environment = "capture"
log_user_prompt = true
# Undocumented in 0.160, and the only way the assistant's reply reaches telemetry.
log_agent_responses = true
exporter = {{ otlp-http = {{ endpoint = "{endpoint}/v1/logs", protocol = "binary" }} }}
trace_exporter = {{ otlp-http = {{ endpoint = "{endpoint}/v1/traces", protocol = "binary" }} }}
metrics_exporter = {{ otlp-http = {{ endpoint = "{endpoint}/v1/metrics", protocol = "binary" }} }}
"""
        )

    def turn(self, run: Run, index: int, prompt: str) -> list[str]:
        resume = ["resume", "--last"] if index else []
        return [self.binary(), "exec", *resume, "--skip-git-repo-check", prompt]


CLIS: dict[str, Cli] = {cli.producer: cli for cli in (ClaudeCode(), Codex())}


def version(cli: Cli) -> str:
    out = subprocess.run(
        [cli.binary(), "--version"], capture_output=True, text=True, check=False
    )
    return out.stdout.strip() or out.stderr.strip()


def without_host_name(raw: bytes) -> bytes:
    """Replace this machine's host name, which Codex reports as ``host.name``, at equal length."""
    host = socket.gethostname().split(".")[0].encode()
    if len(host) < 4:
        return raw
    return raw.replace(host, b"sideseat-host"[: len(host)].ljust(len(host), b"-"))


def capture_one(cli: Cli, scenario: Scenario, mode: str, record: bool) -> bool:
    cassette = HERE / cli.producer / "cassettes" / f"{scenario.name}.json"
    if not record and not cassette.exists():
        print(f"[cli] {cli.producer}/{mode}/{scenario.name}: no cassette at {cassette}")
        return False
    label = f"{cli.producer}/{mode}/{scenario.name}"
    root = SCRATCH / cli.producer
    shutil.rmtree(root, ignore_errors=True)
    workspace, home = root / scenario.name, root / "home"
    workspace.mkdir(parents=True)
    home.mkdir()
    (workspace / "forecast.json").write_text(forecast())

    staging = Path(tempfile.mkdtemp(prefix=f"capture-{cli.producer}-{scenario.name}-"))
    recorder = capture._Recorder
    recorder.out, recorder.forward, recorder.count, recorder.counts = (
        staging,
        None,
        0,
        {},
    )
    recorder.pins = capture.Pins()
    server = ThreadingHTTPServer(("127.0.0.1", 0), recorder)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    telemetry = f"http://127.0.0.1:{server.server_address[1]}"
    action = "recording" if record else "replaying"
    # The requests the CLI sent the model on this run, as every capture path records them: a fixture's
    # request truth comes from its own run, since the CLI writes the date, the workspace and its token
    # budget into what the model reads.
    request_log = staging / "model-requests.jsonl"
    previous_log = os.environ.get(transcript.ENV)
    os.environ[transcript.ENV] = str(request_log)
    print(f"[cli] {label}: {version(cli)} ({action} model traffic)")
    ok, expired = True, False
    try:
        with CliProxy(cassette, record=record) as proxy:
            run = Run(scenario, mode, workspace, home, telemetry, proxy.url)
            cli.prepare(run)
            env = cli.environment(run)
            for index, prompt in enumerate(scenario.turns):
                result = subprocess.run(
                    cli.turn(run, index, prompt),
                    cwd=workspace,
                    env=env,
                    stdin=subprocess.DEVNULL,
                    capture_output=True,
                    text=True,
                    timeout=600,
                    check=False,
                )
                print(
                    f"  [turn {index + 1}] exit {result.returncode}: {result.stdout.strip()[-400:]}"
                )
                if result.returncode != 0:
                    print(result.stderr[-2000:], file=sys.stderr)
                    ok = False
                    break
        # A 403 ExpiredToken answer is recorded like any other; the run it belongs to is void.
        expired = any(
            item["status"] == 403 and b"ExpiredToken" in base64.b64decode(item["body"])
            for item in proxy._recorded
        )
        if proxy.misses:
            print(
                f"[cli] {label}: requests the cassette has no answer for: {proxy.misses}"
            )
            ok = False
    finally:
        # Exporters flush on exit, but a slow flush can still be in flight.
        threading.Event().wait(2)
        server.shutdown()
        shutil.rmtree(root, ignore_errors=True)
        if previous_log is None:
            os.environ.pop(transcript.ENV, None)
        else:
            os.environ[transcript.ENV] = previous_log

    payloads = sorted(staging.glob("req-*")) + sorted(staging.glob("logs-*"))
    leaked = next(
        ((p.name, k) for p in payloads if (k := capture.credential_in(p.read_bytes()))),
        None,
    )
    if (
        leaked
        or expired
        or not ok
        or not any(p.name.startswith("req-") for p in payloads)
    ):
        reason = (
            f"{leaked[0]} holds {leaked[1]}"
            if leaked
            else "expired credentials"
            if expired
            else "failed"
        )
        print(f"[cli] {label}: DISCARDED ({reason}); nothing written")
        shutil.rmtree(staging)
        if record and cassette.exists() and (expired or not ok):
            cassette.unlink()
        return False
    for payload in payloads:
        payload.write_bytes(without_host_name(payload.read_bytes()))
    target = capture.FIXTURES / cli.producer / mode / scenario.name
    target.mkdir(parents=True, exist_ok=True)
    for stale in [*target.glob("req-*"), *target.glob("logs-*")]:
        stale.unlink()
    for payload in payloads:
        shutil.move(payload, target / payload.name)
    (target / transcript.FILENAME).write_text(
        json.dumps(
            transcript.finish(
                request_log, lambda raw: capture.anonymise(raw, recorder.pins)
            ),
            indent=1,
        )
        + "\n"
    )
    shutil.rmtree(staging)
    print(f"[cli] {label}: {len(payloads)} export(s) written")
    return True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("cli", choices=sorted(CLIS))
    parser.add_argument(
        "scenario", nargs="*", help=f"default: all of {', '.join(SCENARIOS)}"
    )
    parser.add_argument(
        "--mode", help="one telemetry mode; default: every mode the CLI has"
    )
    parser.add_argument(
        "--offline", action="store_true", help="replay committed cassettes only"
    )
    args = parser.parse_args()
    cli = CLIS[args.cli]
    if unknown := [name for name in args.scenario if name not in SCENARIOS]:
        parser.error(
            f"unknown scenario(s) {unknown}; choose from {', '.join(SCENARIOS)}"
        )
    modes = (args.mode,) if args.mode else cli.modes
    failed = []
    for name in args.scenario or list(SCENARIOS):
        for position, mode in enumerate(modes):
            # The first mode records, the others replay, so every mode holds one conversation.
            if not capture_one(
                cli, SCENARIOS[name], mode, record=not args.offline and position == 0
            ):
                failed.append(f"{cli.producer}/{mode}/{name}")
    if failed:
        raise SystemExit(f"[cli] failed: {', '.join(failed)}")


if __name__ == "__main__":
    main()
