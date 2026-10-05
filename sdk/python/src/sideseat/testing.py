"""Helpers for testing code that uses SideSeat.

::

    from sideseat.testing import capture

    def test_agent_reports_its_session():
        with capture(integrations=[]) as spans:
            with sideseat.session("s-1"):
                run_agent()
        assert all(s.attributes["session.id"] == "s-1" for s in spans.finished())
"""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from typing import Any

from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter

import sideseat


class CapturedSpans:
    """Finished spans recorded during a :func:`capture` block."""

    def __init__(self, exporter: InMemorySpanExporter) -> None:
        self._exporter = exporter

    def finished(self) -> tuple[ReadableSpan, ...]:
        return tuple(self._exporter.get_finished_spans())

    def named(self, name: str) -> list[ReadableSpan]:
        return [span for span in self.finished() if span.name == name]

    def clear(self) -> None:
        self._exporter.clear()


@contextmanager
def capture(**init_kwargs: Any) -> Iterator[CapturedSpans]:
    """Initialize SideSeat without network export and record finished spans in memory.

    OpenTelemetry allows its global providers to be set once per process, so the providers are reset
    on entry. That makes this suitable for tests only.
    """
    reset_global_providers()
    exporter = InMemorySpanExporter()
    init_kwargs.setdefault("export", False)
    init_kwargs.setdefault("metrics", False)
    init_kwargs.setdefault("logs", False)
    processors = list(init_kwargs.pop("span_processors", ()))
    sideseat.init(span_processors=[*processors, SimpleSpanProcessor(exporter)], **init_kwargs)
    try:
        yield CapturedSpans(exporter)
    finally:
        sideseat.shutdown()
        reset_global_providers()


def reset_global_providers() -> None:
    """Forget OpenTelemetry's global providers, and that SideSeat shut down. For tests only."""
    from opentelemetry import _logs, metrics, trace
    from opentelemetry.util._once import Once

    sideseat._shut_down = False

    trace._TRACER_PROVIDER = None
    trace._TRACER_PROVIDER_SET_ONCE = Once()
    _logs._internal._LOGGER_PROVIDER = None
    _logs._internal._LOGGER_PROVIDER_SET_ONCE = Once()
    metrics._internal._METER_PROVIDER = None
    metrics._internal._METER_PROVIDER_SET_ONCE = Once()
