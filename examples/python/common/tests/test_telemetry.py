"""Regression tests for the shared sample telemetry setup."""

from typing import Any

import sideseat
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import (
    InMemorySpanExporter,
)

from common.telemetry import NativeTraceClient, setup_base_telemetry


def test_sideseat_mode_does_not_run_native_instrumentor(monkeypatch: Any) -> None:
    """SideSeat auto-instruments the framework, so the native callback must stay idle."""
    instrumentor_calls = 0

    class FakeTelemetry:
        def setup_console_exporter(self) -> "FakeTelemetry":
            return self

    class FakeSideSeat:
        def __init__(self, *, framework: str | list[str] | None) -> None:
            self.framework = framework
            self.telemetry = FakeTelemetry()

    def native_instrumentor(provider: Any = None) -> None:
        nonlocal instrumentor_calls
        instrumentor_calls += 1

    monkeypatch.setattr(sideseat, "SideSeat", FakeSideSeat)

    client = setup_base_telemetry(
        instrumentor=native_instrumentor,
        use_sideseat=True,
        framework="autogen",
    )

    assert isinstance(client, FakeSideSeat)
    assert client.framework == "autogen"
    assert instrumentor_calls == 0


def test_native_trace_client_records_session_and_user() -> None:
    """The native control path must preserve the grouping attributes used by SideSeat."""
    exporter = InMemorySpanExporter()
    provider = TracerProvider()
    provider.add_span_processor(SimpleSpanProcessor(exporter))
    client = NativeTraceClient(provider, "native-test")

    with client.trace("conversation", session_id="session-1", user_id="user-1"):
        pass

    spans = exporter.get_finished_spans()
    assert len(spans) == 1
    assert spans[0].name == "conversation"
    assert spans[0].attributes["session.id"] == "session-1"
    assert spans[0].attributes["user.id"] == "user-1"
    client.shutdown()
