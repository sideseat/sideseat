"""The SDK contract's lifecycle, export, and error rules, through the public API."""

from __future__ import annotations

import importlib
import logging
import subprocess
import sys
import textwrap
import threading
import time
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest
from opentelemetry import trace as otel_trace
from opentelemetry.sdk.trace import ReadableSpan, Span, SpanProcessor, TracerProvider

import sideseat
from sideseat.errors import ConfigurationError, IntegrationError
from sideseat.integrations import Integration, SetupContext
from sideseat.testing import capture


class _Receiver:
    """A local OTLP/HTTP endpoint that records each request's path and headers."""

    def __init__(self) -> None:
        self.requests: list[tuple[str, Any]] = []
        receiver = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                self.rfile.read(int(self.headers.get("Content-Length", 0)))
                receiver.requests.append((self.path, self.headers))
                self.send_response(200)
                self.send_header("Content-Length", "0")
                self.end_headers()

            def log_message(self, *args: Any) -> None:
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(
            target=self.server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True
        ).start()

    def paths(self) -> set[str]:
        return {path for path, _ in self.requests}


@pytest.fixture
def receiver() -> Iterator[_Receiver]:
    r = _Receiver()
    yield r
    r.server.shutdown()


class _Framework(Integration):
    name = "framework"
    packages = ("opentelemetry-sdk",)


def test_every_signal_reaches_its_endpoint_with_exactly_one_credential(
    receiver: _Receiver, monkeypatch: pytest.MonkeyPatch
) -> None:
    from opentelemetry import _logs, metrics

    monkeypatch.setenv("OTEL_EXPORTER_OTLP_HEADERS", "authorization=stale,x-team=a")
    sideseat.init(endpoint=receiver.url, project="p", api_key="k", integrations=[])
    with sideseat.span("work"):
        pass
    metrics.get_meter("test").create_counter("tokens").add(1)
    _logs.get_logger("test").emit(body="hello")
    assert sideseat.flush(10_000) is True

    assert {"/otel/p/v1/traces", "/otel/p/v1/logs", "/otel/p/v1/metrics"} <= receiver.paths()
    for _, headers in receiver.requests:
        assert headers.get_all("Authorization") == ["Bearer k"]
        assert headers["x-team"] == "a"
    assert sideseat.shutdown(10_000) is True


def test_disabled_records_nothing_even_when_another_library_configured_otel() -> None:
    from opentelemetry.sdk.trace.export import SimpleSpanProcessor
    from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter

    exporter = InMemorySpanExporter()
    application = TracerProvider()
    application.add_span_processor(SimpleSpanProcessor(exporter))
    otel_trace.set_tracer_provider(application)

    client = sideseat.init(disabled=True)
    with sideseat.trace("t", session_id="s") as root, sideseat.span("c") as child:
        assert not root.is_recording() and not child.is_recording()
    with client.get_tracer("framework").start_as_current_span("f") as span:
        assert not span.is_recording()
    assert exporter.get_finished_spans() == ()


@pytest.mark.parametrize("session_id", [None, ""])
def test_a_session_needs_a_session_id(session_id: Any) -> None:
    sideseat.init(integrations=[], export=False)
    with pytest.raises(ValueError, match="session_id"), sideseat.session(session_id):
        pass


def test_otel_resource_attributes_sit_under_the_sdk_and_explicit_attributes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv(
        "OTEL_RESOURCE_ATTRIBUTES",
        "team=env,region=eu,service.name=env-svc,telemetry.sdk.name=other,sideseat.framework=x",
    )
    with capture(
        integrations=[_Framework()], service_name="svc", resource_attributes={"team": "arg"}
    ) as spans:
        with sideseat.span("work"):
            pass
    resource = spans.finished()[0].resource.attributes
    assert resource["region"] == "eu"
    assert resource["team"] == "arg"
    assert resource["service.name"] == "svc"
    assert resource["telemetry.sdk.name"] == "sideseat"
    assert resource["sideseat.framework"] == "framework"


def test_without_an_integration_the_resource_names_no_framework() -> None:
    with capture(integrations=[]) as spans:
        with sideseat.span("work"):
            pass
    resource = spans.finished()[0].resource.attributes
    assert resource["service.name"] == "sideseat-app"
    assert "sideseat.framework" not in resource
    assert "sideseat.integrations" not in resource


class _BrokenPrepare(_Framework):
    def prepare(self, ctx: SetupContext) -> None:
        raise ImportError("no instrumentation")


def test_a_skipped_detected_integration_does_not_describe_the_service(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # `sideseat._client` the attribute is the module-level client, not the module.
    client_module = importlib.import_module("sideseat._client")
    monkeypatch.setattr(client_module, "resolve", lambda spec: ([_BrokenPrepare()], False))
    with capture() as spans:
        assert sideseat.get_client().integrations == ()
        with sideseat.span("work"):
            pass
    resource = spans.finished()[0].resource.attributes
    assert resource["service.name"] == "sideseat-app"
    assert "sideseat.framework" not in resource


def test_a_requested_integration_with_nothing_to_import_still_needs_its_package() -> None:
    with pytest.raises(IntegrationError, match="langflow"):
        sideseat.init(integrations=["langflow"], export=False)


def test_a_requested_integration_needs_the_packages_of_its_extra() -> None:
    class NeedsExtra(_Framework):
        extra = "crewai"

    with pytest.raises(IntegrationError, match=r"openinference-instrumentation-crewai.*sideseat\["):
        sideseat.init(integrations=[NeedsExtra()], export=False)


class _SlowFlush(_Framework):
    def flush(self, timeout_millis: int) -> bool:
        time.sleep(0.5)
        return True


def test_flush_reports_failure_once_its_timeout_passes() -> None:
    sideseat.init(integrations=[_SlowFlush()], export=False)
    started = time.monotonic()
    assert sideseat.flush(100) is False
    assert time.monotonic() - started < 0.4


class _FailingFlush(_Framework):
    def flush(self, timeout_millis: int) -> bool:
        raise RuntimeError("exporter exploded")


def test_a_failing_flush_is_reported_not_raised() -> None:
    sideseat.init(integrations=[_FailingFlush()], export=False)
    assert sideseat.flush() is False


class _SlowShutdown(SpanProcessor):
    def __init__(self) -> None:
        self.stopped = threading.Event()

    def on_end(self, span: ReadableSpan) -> None:
        pass

    def shutdown(self) -> None:
        time.sleep(1)
        self.stopped.set()


def test_shutdown_is_bounded_by_its_timeout_and_reports_the_same_result_again() -> None:
    client = sideseat.init(integrations=[], export=False, span_processors=[_SlowShutdown()])
    started = time.monotonic()
    assert client.shutdown(100) is False
    assert time.monotonic() - started < 0.6
    assert client.shutdown(100) is False


class _Recorder(SpanProcessor):
    def __init__(self) -> None:
        self.shut_down = False

    def on_start(self, span: Span, parent_context: Any = None) -> None:
        pass

    def on_end(self, span: ReadableSpan) -> None:
        pass

    def shutdown(self) -> None:
        self.shut_down = True


def test_shutdown_stops_what_it_added_to_an_application_provider() -> None:
    application = TracerProvider()
    otel_trace.set_tracer_provider(application)
    recorder = _Recorder()
    client = sideseat.init(integrations=[], export=False, span_processors=[recorder])
    assert client.tracer_provider is application
    assert client.shutdown() is True
    assert recorder.shut_down


def test_init_after_shutdown_is_an_error() -> None:
    sideseat.init(integrations=[], export=False)
    assert sideseat.shutdown() is True
    with pytest.raises(ConfigurationError, match=r"after sideseat\.shutdown"):
        sideseat.init(integrations=[], export=False)


def test_an_application_meter_provider_is_reported_rather_than_silently_skipped(
    caplog: pytest.LogCaptureFixture,
) -> None:
    from opentelemetry import metrics
    from opentelemetry.sdk.metrics import MeterProvider

    metrics.set_meter_provider(MeterProvider())
    with caplog.at_level(logging.WARNING, logger="sideseat"):
        sideseat.init(integrations=[], logs=False)
    assert "meter provider" in caplog.text


_EXIT_SCRIPT = """
import os, sys, time
import sideseat
from sideseat.integrations import Integration

class Marker(Integration):
    name = "marker"
    packages = ("opentelemetry-sdk",)

    def shutdown(self):
        open(sys.argv[1], "w").write("stopped")

sideseat.init(integrations=[Marker()], export=False)
if sys.argv[2] == "sigterm":
    import signal
    os.kill(os.getpid(), signal.SIGTERM)
    time.sleep(10)
"""


@pytest.mark.parametrize(
    "ending",
    [
        "return",
        pytest.param(
            "sigterm",
            marks=pytest.mark.skipif(sys.platform == "win32", reason="POSIX signal semantics"),
        ),
    ],
)
def test_the_pipeline_shuts_down_when_the_process_ends(tmp_path: Path, ending: str) -> None:
    marker = tmp_path / "stopped"
    result = subprocess.run(
        [sys.executable, "-c", textwrap.dedent(_EXIT_SCRIPT), str(marker), ending],
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert marker.read_text() == "stopped", result.stderr
    if ending == "sigterm":
        import signal

        # The process still ends the way SIGTERM ends it.
        assert result.returncode == -signal.SIGTERM


def _fake_logfire(monkeypatch: pytest.MonkeyPatch) -> dict[str, Any]:
    """A stand-in for ``logfire`` that builds providers from what ``configure`` receives."""
    import types

    from opentelemetry.sdk._logs import LoggerProvider
    from opentelemetry.sdk.metrics import MeterProvider
    from opentelemetry.sdk.resources import Resource

    seen: dict[str, Any] = {}

    class Options:
        def __init__(self, **kwargs: Any) -> None:
            self.__dict__.update(kwargs)

    def configure(**kwargs: Any) -> Any:
        seen.update(kwargs)
        resource = Resource(kwargs["resource_attributes"])
        logs = LoggerProvider(resource=resource)
        for processor in kwargs["advanced"].log_record_processors:
            logs.add_log_record_processor(processor)
        options = kwargs["metrics"]
        readers = list(options.additional_readers) if options else []
        meters = MeterProvider(resource=resource, metric_readers=readers)
        traces = TracerProvider(resource=resource)
        seen["providers"] = (traces, logs, meters)
        config = types.SimpleNamespace(
            get_tracer_provider=lambda: traces,
            get_logger_provider=lambda: logs,
            get_meter_provider=lambda: meters,
        )
        return types.SimpleNamespace(config=config)

    module = types.ModuleType("logfire")
    module.configure = configure  # type: ignore[attr-defined]
    module.MetricsOptions = Options  # type: ignore[attr-defined]
    module.AdvancedOptions = Options  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "logfire", module)
    from sideseat.integrations.logfire import Logfire

    monkeypatch.setattr(Logfire, "installed_package", classmethod(lambda cls: ("logfire", "5")))
    monkeypatch.setattr(importlib.import_module("sideseat._client"), "_installed", lambda n: True)
    return seen


def test_an_owning_integration_exports_logs_and_metrics_through_its_own_providers(
    receiver: _Receiver, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen = _fake_logfire(monkeypatch)
    client = sideseat.init(
        endpoint=receiver.url, integrations=["logfire"], resource_attributes={"n": 3}
    )
    traces, logs, meters = seen["providers"]
    assert client.tracer_provider is traces
    meters.get_meter("logfire").create_counter("gen_ai.client.token.usage").add(5)
    logs.get_logger("logfire").emit(body="event")
    with sideseat.span("work"):
        pass
    assert sideseat.flush(10_000) is True
    assert {"/otel/default/v1/traces", "/otel/default/v1/logs", "/otel/default/v1/metrics"} <= (
        receiver.paths()
    )
    # The resource keeps its types rather than travelling through a string variable.
    assert seen["resource_attributes"]["n"] == 3
    assert tuple(seen["resource_attributes"]["sideseat.integrations"]) == ("logfire",)
    assert sideseat.shutdown(10_000) is True


class _FailingShutdown(_Framework):
    def shutdown(self) -> None:
        raise RuntimeError("cannot stop")


def test_one_failing_shutdown_step_does_not_stop_the_rest() -> None:
    recorder = _Recorder()
    client = sideseat.init(
        integrations=[_FailingShutdown()], export=False, span_processors=[recorder]
    )
    assert client.shutdown() is False
    assert recorder.shut_down
