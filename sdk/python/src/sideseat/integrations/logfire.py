"""Logfire, and the providers and frameworks Logfire instruments.

Logfire builds its own tracer provider, so these integrations own it. SideSeat configures Logfire
with no exporters of its own and attaches SideSeat's processors and exporter to the provider Logfire
made.
"""

from __future__ import annotations

import hashlib
import logging
import threading
import time
from typing import Any, ClassVar
from urllib.parse import quote

from opentelemetry import trace as otel_trace
from opentelemetry.sdk.trace import SpanProcessor, TracerProvider
from opentelemetry.trace import SpanContext

from sideseat.integrations._base import Integration, SetupContext
from sideseat.integrations._logfire_compat import apply_after, apply_before
from sideseat.integrations._util import temporary_env, without_otlp_exporter_env

logger = logging.getLogger("sideseat")


class LogfireIntegration(Integration):
    """Configures Logfire as the tracer provider owner, then runs one ``logfire.instrument_*``."""

    owns_tracer_provider = True
    extra = "logfire"
    #: Suffix of the ``logfire.instrument_<suffix>`` call; ``None`` configures Logfire only.
    instrument_method: ClassVar[str | None] = None

    def create_tracer_provider(self, ctx: SetupContext) -> TracerProvider:
        import logfire

        resource = ",".join(
            f"{quote(str(key), safe='')}={quote(str(value), safe='')}"
            for key, value in ctx.resource.attributes.items()
        )
        # Logfire reads OTLP variables at configure time and would add exporters of its own; the
        # resource travels in OTEL_RESOURCE_ATTRIBUTES because configure takes no resource argument.
        with without_otlp_exporter_env(), temporary_env({"OTEL_RESOURCE_ATTRIBUTES": resource}):
            logfire.configure(
                service_name=ctx.service_name,
                service_version=ctx.service_version,
                send_to_logfire=False,
                console=False,
            )
        provider = otel_trace.get_tracer_provider()
        if not hasattr(provider, "add_span_processor"):
            raise RuntimeError("Logfire did not install an SDK tracer provider")
        return provider  # type: ignore[return-value]

    def span_processors(self, ctx: SetupContext) -> tuple[SpanProcessor, ...]:
        return (StreamingResponseReparenter(),)

    def instrument(self, ctx: SetupContext) -> None:
        if self.instrument_method is None:
            return
        import logfire

        apply_before(self.instrument_method)
        getattr(logfire, f"instrument_{self.instrument_method}")()
        apply_after(self.instrument_method)


class Logfire(LogfireIntegration):
    name = "logfire"
    packages = ("logfire",)
    # Several provider extras install Logfire, so its presence says nothing about the application.
    detectable = False


class OpenAIAgents(LogfireIntegration):
    name = "openai-agents"
    packages = ("openai-agents",)
    extra = "openai-agents"
    instrument_method = "openai_agents"


class PydanticAI(LogfireIntegration):
    name = "pydantic-ai"
    packages = ("pydantic-ai", "pydantic-ai-slim")
    extra = "pydantic-ai"
    instrument_method = "pydantic_ai"


class OpenAI(LogfireIntegration):
    name = "openai"
    packages = ("openai",)
    detectable = False
    extra = "openai"
    instrument_method = "openai"


class Anthropic(LogfireIntegration):
    name = "anthropic"
    packages = ("anthropic",)
    detectable = False
    extra = "anthropic"
    instrument_method = "anthropic"


class GoogleGenAI(LogfireIntegration):
    name = "google-genai"
    packages = ("google-genai",)
    detectable = False
    extra = "google-genai"
    instrument_method = "google_genai"


class VertexAI(GoogleGenAI):
    name = "vertex-ai"
    extra = "vertex-ai"


class StreamingResponseReparenter(SpanProcessor):
    """Reparents logfire streaming response logs under their request span's trace.

    Logfire captures the OpenTelemetry context before it creates the request span. When the request
    has no parent, that context is empty, and once the stream completes the response log is emitted
    under it - as the root of a new, unrelated trace. The server cannot reconnect the two halves
    after ingestion, so this ``on_end`` processor does it before export: it remembers each request
    span and rewrites the matching response log's trace and parent to the request's.

    Detection (definitive logfire signals):
      Request span: ``logfire.span_type="span"`` + ``request_data`` present + no output carrier
      Response log: ``logfire.span_type="log"`` + ``request_data`` present

    Note: Chat Completions streaming logs carry ``response_data``; Responses API
    streaming logs carry ``events`` instead. Logfire 6 uses
    ``gen_ai.output.messages`` for completed non-streaming spans. All are handled.

    Matching: SHA-256 of ``request_data`` plus ``gen_ai.input.messages`` when present. Some Logfire
    versions reduce ``request_data`` to the model name, so it alone cannot tell two calls to the
    same model apart; the input messages, present on both halves, can. FIFO queues per key keep
    concurrent identical streaming requests paired in order.

    Mutation: replaces ``ReadableSpan._context`` and ``._parent``.
    ``ReadableSpan`` has no ``__setattr__`` override, and ``BatchSpanProcessor``
    stores a reference (not copy), so the exporter sees the updated context.

    Ordering: must be added to the TracerProvider BEFORE BatchSpanProcessor
    so mutation completes before the span enters the export queue.
    ``SynchronousMultiSpanProcessor.on_end`` iterates processors in insertion order.

    Non-Logfire spans carry no ``logfire.span_type`` attribute and pass through untouched.
    """

    _TTL = 60.0
    _MAX_PENDING = 1000

    def __init__(self) -> None:
        self._pending: dict[bytes, list[tuple[SpanContext, float]]] = {}
        self._lock = threading.Lock()

    def on_start(self, span: Any, parent_context: Any = None) -> None:
        pass

    def on_end(self, span: Any) -> None:
        attrs = span.attributes
        if not attrs:
            return

        span_type = attrs.get("logfire.span_type")
        if not isinstance(span_type, str):
            return

        request_data = attrs.get("request_data")
        if not isinstance(request_data, str):
            return

        input_messages = attrs.get("gen_ai.input.messages")
        if not isinstance(input_messages, str):
            input_messages = None

        has_output = isinstance(attrs.get("response_data"), str) or isinstance(
            attrs.get("gen_ai.output.messages"), str
        )
        if span_type == "span" and not has_output:
            self._store_request(request_data, input_messages, span)
        elif span_type == "log":
            self._reparent_response(request_data, input_messages, span)

    def _store_request(self, request_data: str, input_messages: str | None, span: Any) -> None:
        """Remember the request span's trace context for later matching."""
        key = _make_key(request_data, input_messages)
        ctx = span.context
        entry = (ctx, time.monotonic())
        with self._lock:
            self._pending.setdefault(key, []).append(entry)
            self._cleanup()

    def _reparent_response(self, request_data: str, input_messages: str | None, span: Any) -> None:
        """Rewrite response log's trace/parent to match the request span."""
        key = _make_key(request_data, input_messages)
        with self._lock:
            entries = self._pending.get(key)
            if not entries:
                return
            entry = entries.pop(0)
            if not entries:
                del self._pending[key]

        parent_context, _ = entry
        old_ctx = span.context
        old_parent = getattr(span, "_parent", None)
        if (
            old_ctx.trace_id == parent_context.trace_id
            and old_parent is not None
            and old_parent.span_id == parent_context.span_id
        ):
            return

        try:
            span._context = SpanContext(
                trace_id=parent_context.trace_id,
                span_id=old_ctx.span_id,
                is_remote=False,
                trace_flags=parent_context.trace_flags,
                trace_state=parent_context.trace_state,
            )
            span._parent = parent_context
        except Exception:
            logger.debug("Failed to reparent logfire streaming response", exc_info=True)
            with self._lock:
                self._pending.setdefault(key, []).insert(0, entry)

    def shutdown(self) -> None:
        with self._lock:
            self._pending.clear()

    def force_flush(self, timeout_millis: int = 30000) -> bool:
        return True

    def _cleanup(self) -> None:
        """Remove stale entries and enforce size cap. Must be called with lock held."""
        now = time.monotonic()
        total = 0
        for key in list(self._pending):
            entries = self._pending[key]
            entries[:] = [e for e in entries if now - e[1] <= self._TTL]
            if not entries:
                del self._pending[key]
            else:
                total += len(entries)

        while total > self._MAX_PENDING:
            oldest_key = min(self._pending, key=lambda k: self._pending[k][0][1])
            self._pending[oldest_key].pop(0)
            if not self._pending[oldest_key]:
                del self._pending[oldest_key]
            total -= 1


def _make_key(request_data: str, input_messages: str | None) -> bytes:
    digest = hashlib.sha256()
    for value in (request_data, input_messages):
        if value is None:
            digest.update(b"\x00")
        else:
            encoded = value.encode()
            digest.update(b"\x01")
            digest.update(len(encoded).to_bytes(8, "big"))
            digest.update(encoded)
    return digest.digest()
