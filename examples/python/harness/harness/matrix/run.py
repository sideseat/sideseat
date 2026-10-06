"""Replay one scenario in a variant's environment and record what it exports."""

from __future__ import annotations

import json
import os
import shlex
import shutil
import signal
import subprocess
import tempfile
import threading
from dataclasses import dataclass, field
from http.server import ThreadingHTTPServer
from pathlib import Path

from harness import transcript
from harness.capture import Pins, Suite, _Recorder, credential_in, uses_fake_model
from harness.matrix.environment import executable
from harness.proxy import ModelProxy, client_environment


@dataclass
class Replay:
    """What one replayed run did; ``ok`` only if it is a faithful copy of the recorded conversation."""

    scenario: str
    staging: Path
    returncode: int = 0
    exact: int = 0
    by_order: int = 0
    misses: list[str] = field(default_factory=list)
    unanswered: list[str] = field(default_factory=list)
    leaked: str | None = None
    output: str = ""

    @property
    def requests(self) -> list[Path]:
        return sorted(self.staging.glob("req-*"))

    @property
    def problems(self) -> list[str]:
        found = []
        if self.returncode != 0:
            found.append(f"the scenario exited with {self.returncode}")
        if self.misses:
            found.append(f"requests the cassette has no answer for: {self.misses}")
        if self.unanswered:
            # Answers were matched by arrival order, which is sound only if the run asked for all of them.
            found.append(f"recorded answers no request asked for: {self.unanswered}")
        if self.leaked:
            found.append(f"a payload holds {self.leaked}")
        if not self.requests:
            found.append("no trace export was recorded")
        return found

    @property
    def ok(self) -> bool:
        return not self.problems

    def discard(self) -> None:
        shutil.rmtree(self.staging, ignore_errors=True)


def replay(
    suite: Suite,
    environment: Path,
    scenario: str,
    *,
    mode: str = "native",
    env: dict[str, str] | None = None,
    timeout: float = 600,
    cassettes: Path | None = None,
    live: bool = False,
    allow_hosts: tuple[str, ...] = (),
) -> Replay:
    """Run ``scenario`` from ``environment`` against the suite's committed cassette, offline.

    ``cassettes`` is the directory to replay from (default: the suite's). ``live`` records into it
    instead, through the recording proxy on the ambient AWS credentials, for a release whose model
    traffic no committed cassette holds.

    The answer to each request is the recorded one for an identical request, else the next recorded one
    on the same method and path: an older release serialises its requests differently, so most matches are
    by order. That is why the run must also consume every recorded answer - a run that asked fewer
    questions took a different course, and its telemetry is not the recorded conversation.
    """
    staging = Path(
        tempfile.mkdtemp(prefix=f"matrix-{suite.manifest['producer']}-{scenario}-")
    )
    result = Replay(scenario, staging)
    # What this release's run sent the model, beside its telemetry: the request truth is per fixture, and
    # a historical release serialises the same conversation its own way.
    request_log = staging / "model-requests.jsonl"
    previous_log = os.environ.get(transcript.ENV)
    os.environ[transcript.ENV] = str(request_log)
    _Recorder.out, _Recorder.forward, _Recorder.count = staging, None, 0
    _Recorder.counts = {}
    _Recorder.pins = Pins()
    recorder = ThreadingHTTPServer(("127.0.0.1", 0), _Recorder)
    threading.Thread(target=recorder.serve_forever, daemon=True).start()
    arguments = [scenario] + (["--sideseat"] if mode == "sdk" else [])
    run_env = {
        **os.environ,
        **(env or {}),
        transcript.ENV: str(request_log),
        "SIDESEAT_ENDPOINT": f"http://127.0.0.1:{recorder.server_address[1]}",
        "SIDESEAT_PROJECT_ID": "default",
    }
    run_env.pop("OTEL_EXPORTER_OTLP_ENDPOINT", None)
    # Every model request goes to the local proxy, which alone reaches Bedrock (in live mode). A release
    # that ignores the client it is given would otherwise call its provider's public API from here, so
    # the scenario gets a proxy that refuses everything but loopback.
    run_env.update(
        {
            name: "http://127.0.0.1:9"
            for name in (
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            )
        }
    )
    run_env["NO_PROXY"] = run_env["no_proxy"] = ",".join(
        ["127.0.0.1", "localhost", *allow_hosts]
    )
    # The variant's interpreter, not the one running the harness: VIRTUAL_ENV would point uv elsewhere.
    run_env.pop("VIRTUAL_ENV", None)
    if suite.language == "javascript":
        # The suite's directory inside the variant's copy of the npm project; npm passes it as INIT_CWD.
        command = ["npm", "run", "--silent", "sample", "--", *arguments]
        workdir = environment / suite.root.name
    elif suite.language == "go":
        command = ["go", "run", ".", *arguments]
        workdir = environment / suite.root.name
    elif suite.language == "java":
        from harness.matrix.gradle import GRADLE_FLAGS

        command = [
            str(environment / "gradlew"),
            *GRADLE_FLAGS,
            "run",
            f"--args={shlex.join(arguments)}",
        ]
        workdir = environment / suite.root.name
    else:
        command = [str(executable(environment, "sample")), *arguments]
        workdir = suite.root
    try:
        if uses_fake_model(suite, None):
            if suite.language != "python":
                # A Python suite starts its fake in-process; a suite in another language is pointed at one.
                from harness import fakes
                from harness.clients import FAKE_PATHS
                from harness.models import resolve

                surface = resolve(suite.manifest.get("default-model", "")).surface
                run_env[f"{surface.upper().replace('-', '_')}_URL"] = (
                    fakes.start(surface) + FAKE_PATHS[surface]
                )
            completed = _run(command, workdir, run_env, timeout)
        else:
            cassette = (cassettes or suite.root / "cassettes") / f"{scenario}.json"
            if not live and not cassette.exists():
                result.returncode = -1
                result.output = f"no cassette at {cassette}"
                return result
            with ModelProxy(cassette, record=live) as proxy:
                run_env.update(client_environment(proxy.url))
                completed = _run(command, workdir, run_env, timeout)
            result.exact, result.by_order = proxy.exact, proxy.by_order
            result.misses, result.unanswered = proxy.misses, proxy.unanswered()
        result.returncode, result.output = completed
    finally:
        recorder.shutdown()
        if previous_log is None:
            os.environ.pop(transcript.ENV, None)
        else:
            os.environ[transcript.ENV] = previous_log
    (staging / transcript.FILENAME).write_text(
        json.dumps(transcript.finish(request_log), indent=1) + "\n"
    )
    request_log.unlink(missing_ok=True)
    for path in sorted(staging.iterdir()):
        if kind := credential_in(path.read_bytes()):
            result.leaked = f"{kind} ({path.name})"
            break
    return result


def _run(
    command: list[str], cwd: Path, env: dict[str, str], timeout: float
) -> tuple[int, str]:
    """Run a scenario, and on timeout stop everything it started.

    A scenario starts its own children (npm starts tsx, which starts the Claude Code CLI); killing only the
    direct child leaves them holding the output pipes, so the wait for its output would never end. The
    scenario runs in a session of its own, and a timeout kills the whole process group.
    """
    process = subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=os.name != "nt",
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        if os.name != "nt":
            os.killpg(process.pid, signal.SIGKILL)
        else:
            process.kill()
        process.communicate()
        return -9, f"timed out after {timeout:.0f}s"
    return process.returncode, (stdout + stderr)[-4000:]


def commit(result: Replay, target: Path) -> int:
    """Move a faithful run's exports into its fixture directory, replacing the previous capture."""
    assert result.ok, result.problems
    target.mkdir(parents=True, exist_ok=True)
    for pattern in ("req-*", "logs-*"):
        for stale in target.glob(pattern):
            stale.unlink()
    moved = 0
    for payload in sorted(result.staging.iterdir()):
        shutil.move(payload, target / payload.name)
        moved += payload.name.startswith("req-")
    result.discard()
    return moved
