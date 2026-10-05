"""Browser Use: its structure spans come from Laminar, which owns the tracer provider."""

from __future__ import annotations

import os
from collections.abc import Iterator
from contextlib import contextmanager
from typing import Any, cast
from urllib.parse import quote

from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.trace import Span

from sideseat._context import Correlation
from sideseat.integrations._base import Integration, SetupContext
from sideseat.integrations._util import temporary_env


class BrowserUse(Integration):
    name = "browser-use"
    packages = ("browser-use",)
    extra = "browser-use"
    owns_tracer_provider = True

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
        # Laminar builds its exporter from these variables during initialize and never reads them
        # again, so they are scoped to the call. It also raises the attribute count limit for the
        # provider it builds; listing the variable restores the application's value afterwards.
        with temporary_env(
            {
                "OTEL_ATTRIBUTE_COUNT_LIMIT": os.environ.get("OTEL_ATTRIBUTE_COUNT_LIMIT"),
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": ctx.settings.signal_endpoint("traces"),
                "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL": "http/protobuf",
                "OTEL_EXPORTER_OTLP_TRACES_HEADERS": headers or None,
                "LMNR_TRACE_CONTENT": "true" if ctx.capture_content else "false",
            }
        ):
            Laminar.initialize(
                instruments=_instruments(Instruments),
                force_http=True,
                set_global_tracer_provider=True,
            )
        provider = _laminar_provider()
        _apply_resource(provider, ctx.resource)
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

    def shutdown(self) -> None:
        from lmnr import Laminar

        Laminar.shutdown()


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
