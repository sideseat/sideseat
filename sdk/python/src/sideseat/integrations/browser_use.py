"""Browser Use: its structure spans come from Laminar, which owns the tracer provider."""

from __future__ import annotations

import os
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from typing import Any, cast
from urllib.parse import quote

from opentelemetry.sdk.trace import ReadableSpan, TracerProvider
from opentelemetry.sdk.trace.export import SpanExporter, SpanExportResult
from opentelemetry.trace import Span

from sideseat._context import Correlation
from sideseat.integrations._base import Integration, SetupContext
from sideseat.integrations._util import content_switch, temporary_env


class BrowserUse(Integration):
    name = "browser-use"
    packages = ("browser-use",)
    extra = "browser-use"
    owns_tracer_provider = True

    def prepare(self, ctx: SetupContext) -> None:
        # Laminar's instrumentations read these on every model call; its Google GenAI one reads the
        # second spelling.
        content_switch(ctx.settings, "LMNR_TRACE_CONTENT")
        content_switch(ctx.settings, "LAMINAR_TRACE_CONTENT")

    def create_tracer_provider(self, ctx: SetupContext) -> TracerProvider:
        from lmnr import Instruments, Laminar
        from lmnr.sdk.utils import from_env

        if Laminar.is_initialized():
            raise RuntimeError("call sideseat.init before Laminar.initialize")
        for key in ("LMNR_PROJECT_API_KEY", "LMNR_BASE_URL"):
            if from_env(key):
                raise RuntimeError(f"{key} would send browser-use telemetry to Laminar; unset it")

        headers = ",".join(
            f"{quote(k, safe='-._~')}={quote(v, safe='-._~')}"
            for k, v in ctx.settings.headers().items()
        )
        # Laminar builds its exporters itself and accepts none at initialize, so the two classes
        # it instantiates are swapped for the call. Its log exporter derives the logs URL from the
        # traces variable and would post log records to the traces endpoint; SideSeat's log
        # processor goes on Laminar's logger provider instead. With export off, spans are dropped.
        from lmnr.opentelemetry_lib import tracing
        from lmnr.opentelemetry_lib.tracing import processor

        span_exporter = processor.LaminarSpanExporter if ctx.settings.export else _DiscardSpans
        # Laminar builds its exporter from these variables during initialize and never reads them
        # again, so they are scoped to the call. It also raises the attribute count limit for the
        # provider it builds; listing the variable restores the application's value afterwards.
        with (
            _replaced(tracing, "LaminarLogExporter", _discard_logs()),
            _replaced(processor, "LaminarSpanExporter", span_exporter),
            temporary_env(
                {
                    "OTEL_ATTRIBUTE_COUNT_LIMIT": os.environ.get("OTEL_ATTRIBUTE_COUNT_LIMIT"),
                    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": ctx.settings.signal_endpoint("traces"),
                    "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL": "http/protobuf",
                    "OTEL_EXPORTER_OTLP_TRACES_HEADERS": headers or None,
                }
            ),
        ):
            Laminar.initialize(
                instruments=_instruments(Instruments),
                force_http=True,
                set_global_tracer_provider=True,
            )
        provider = _laminar_provider()
        _apply_resource(provider, ctx.resource)
        # Laminar made its logger provider the global one and hands it to its instrumentations.
        # SideSeat stops it with the tracer provider, whether or not it exports logs.
        logger_provider = _laminar_logger_provider()
        log_processor = ctx.log_record_processor()
        if log_processor is not None:
            logger_provider.add_log_record_processor(log_processor)
        ctx.logger_provider = logger_provider
        # Laminar's exporter already sends to SideSeat; SideSeat must not add a second one.
        ctx.notes["exporter_installed"] = True
        return cast(TracerProvider, provider)

    @contextmanager
    def activate(self, span: Span, correlation: Correlation) -> Iterator[None]:
        # Laminar parents its spans from an isolated context of its own, not the OpenTelemetry one.
        from lmnr import Laminar, LaminarSpan

        laminar_span = LaminarSpan(span)
        laminar_span.set_trace_user_id(correlation.user_id)
        laminar_span.set_trace_session_id(correlation.session_id)
        with Laminar.use_span(laminar_span, record_exception=False, set_status_on_exception=False):
            yield

    def flush(self, timeout_millis: int) -> bool:
        from lmnr import Laminar

        return bool(Laminar.flush())


def _instruments(instruments: Any) -> set[Any]:
    """Bubus for Browser Use's event structure, and the client library of every model it wraps.

    Browser Use's own spans hold no model messages; those come from the instrumented client its chat
    model calls, which may be any of these. Laminar skips an instrument whose library is absent.
    """
    return {
        instruments.BUBUS,
        instruments.ANTHROPIC,
        instruments.BEDROCK,
        instruments.GOOGLE_GENAI,
        instruments.GROQ,
        instruments.LITELLM,
        instruments.MISTRAL,
        instruments.OLLAMA,
        instruments.OPENAI,
    }


class _DiscardSpans(SpanExporter):
    """Stands in for Laminar's span exporter when SideSeat's export is off."""

    def __init__(self, **_: Any) -> None:
        pass

    def export(self, spans: Sequence[ReadableSpan]) -> SpanExportResult:
        return SpanExportResult.SUCCESS


def _discard_logs() -> type[Any]:
    """Stands in for Laminar's log exporter; SideSeat's processor exports Laminar's log records.

    Built on demand: the log export names it needs exist only in the OpenTelemetry releases Laminar
    requires, not in every release SideSeat installs beside.
    """
    from opentelemetry.sdk._logs.export import LogRecordExporter, LogRecordExportResult

    class DiscardLogs(LogRecordExporter):
        def __init__(self, **_: Any) -> None:
            pass

        def export(self, batch: Sequence[Any]) -> LogRecordExportResult:
            return LogRecordExportResult.SUCCESS

        def shutdown(self) -> None:
            pass

        def force_flush(self, timeout_millis: int = 30_000) -> bool:
            return True

    return DiscardLogs


@contextmanager
def _replaced(module: Any, name: str, value: Any) -> Iterator[None]:
    original = getattr(module, name)
    setattr(module, name, value)
    try:
        yield
    finally:
        setattr(module, name, original)


def _laminar_logger_provider() -> Any:
    from lmnr.opentelemetry_lib.tracing import TracerWrapper

    provider = getattr(TracerWrapper.instance, "_logger_provider", None)
    if provider is None or not hasattr(provider, "add_log_record_processor"):
        raise RuntimeError("Laminar did not expose the logger provider it created")
    return provider


def _laminar_provider() -> Any:
    # Laminar's public accessor returns a proxy without add_span_processor; the wrapper holds the
    # real one.
    from lmnr.opentelemetry_lib.tracing import TracerWrapper

    wrapper = getattr(TracerWrapper, "instance", None)
    provider = getattr(wrapper, "_tracer_provider", None)
    if provider is None or not hasattr(provider, "add_span_processor"):
        raise RuntimeError("Laminar did not expose the tracer provider it created")
    return provider


def _apply_resource(provider: Any, resource: Any) -> None:
    """Give Laminar's provider, and the tracers it already created, SideSeat's resource.

    Laminar takes no resource at initialize, and each tracer snapshots the provider's resource when
    it is created - Laminar creates its instrumentation tracers during initialize - so both are
    updated before any application span exists.
    """
    from lmnr.opentelemetry_lib.tracing import TracerWrapper

    provider._resource = resource
    with provider._tracers_lock:
        for tracer in provider._tracers.values():
            tracer.resource = resource
    wrapper = TracerWrapper.instance
    wrapper._resource = resource
    logger_provider = getattr(wrapper, "_logger_provider", None)
    if logger_provider is not None:
        logger_provider._resource = resource
