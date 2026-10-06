"""Browser Use's Laminar exports only what SideSeat's settings allow, each signal to its endpoint.

Laminar is a process-wide singleton that cannot be re-initialized, so every case runs in a child
process. The SDK's own environment cannot hold Laminar beside TraceLoop; `make test-sdk-python` runs
this module in the Browser Use example environment, and it skips anywhere Laminar is absent.
"""

from __future__ import annotations

import json
import subprocess
import sys

import pytest

pytest.importorskip("lmnr")

_CHILD = """
import http.server, json, sys, threading

posted = []

class Collector(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", 0)))
        posted.append(self.path)
        self.send_response(200)
        self.send_header("content-type", "application/x-protobuf")
        self.end_headers()

    def log_message(self, *args):
        pass

server = http.server.HTTPServer(("127.0.0.1", 0), Collector)
threading.Thread(target=server.serve_forever, daemon=True).start()

import sideseat
from lmnr import Laminar
from lmnr.opentelemetry_lib.opentelemetry.instrumentation.anthropic.utils import should_send_prompts
from opentelemetry import _logs
from opentelemetry._logs import LogRecord
from opentelemetry.sdk.trace import SpanProcessor

class CountsShutdowns(SpanProcessor):
    shutdowns = 0

    def shutdown(self):
        CountsShutdowns.shutdowns += 1

client = sideseat.init(
    endpoint=f"http://127.0.0.1:{server.server_port}",
    integrations=["browser-use"],
    metrics=False,
    span_processors=[CountsShutdowns()],
    **json.loads(sys.argv[1]),
)
with client.span("step"):
    pass
_logs.get_logger("browser-use-test").emit(LogRecord(body="navigated"))
initialized = Laminar.is_initialized()
sends_prompts = bool(should_send_prompts())
sideseat.shutdown()
print(json.dumps({
    "posted": sorted(posted),
    "laminar": initialized,
    "prompts": sends_prompts,
    "shutdowns": CountsShutdowns.shutdowns,
}))
"""


def _run(**init: object) -> dict[str, object]:
    done = subprocess.run(
        [sys.executable, "-c", _CHILD, json.dumps(init)],
        capture_output=True,
        text=True,
        timeout=120,
        check=True,
    )
    result: dict[str, object] = json.loads(done.stdout.strip().splitlines()[-1])
    return result


def test_spans_go_to_the_traces_endpoint_and_log_records_to_the_logs_endpoint() -> None:
    result = _run(logs=True)
    assert result["posted"] == ["/otel/default/v1/logs", "/otel/default/v1/traces"]
    assert result["prompts"] is True
    # Laminar's providers are stopped once, by SideSeat.
    assert result["shutdowns"] == 1


def test_content_stays_off_for_model_calls_after_init() -> None:
    assert _run(capture_content=False, export=False)["prompts"] is False


def test_nothing_is_exported_with_export_off() -> None:
    assert _run(export=False, logs=True)["posted"] == []


def test_log_records_stay_local_with_logs_off() -> None:
    assert _run(logs=False)["posted"] == ["/otel/default/v1/traces"]


def test_a_disabled_sdk_never_initializes_laminar() -> None:
    result = _run(disabled=True, logs=True)
    assert (result["posted"], result["laminar"]) == ([], False)
