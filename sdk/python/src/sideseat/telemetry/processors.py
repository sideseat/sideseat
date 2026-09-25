"""Span processors for logfire integration."""

from __future__ import annotations

import hashlib
import logging
import threading
import time
from typing import Any

from opentelemetry.sdk.trace import SpanProcessor
from opentelemetry.trace import SpanContext

logger = logging.getLogger("sideseat.telemetry")


class _LogfireStreamingProcessor(SpanProcessor):
    """Reparents logfire streaming response logs under their request span's trace.

    Root cause: logfire's ``llm_provider.py`` calls ``get_context()`` in
    ``_instrumentation_setup()`` *before* the request span is created (line 102
    vs 141).  When there is no parent span, ``get_context()`` captures an empty
    OTel context.  After streaming, ``attach_context(original_context)``
    restores that empty context, so ``logfire.info()`` starts a new root trace
    for the response log instead of continuing the request span's trace.

    Fix: this ``on_end`` processor watches for the two halves — request span
    and response log — and rewrites the response log's trace/parent context to
    match the request span.

    Detection (definitive logfire signals):
      Request span: ``logfire.span_type="span"`` + ``request_data`` present + no output carrier
      Response log: ``logfire.span_type="log"`` + ``request_data`` present

    Note: Chat Completions streaming logs carry ``response_data``; Responses API
    streaming logs carry ``events`` instead. Logfire 6 uses
    ``gen_ai.output.messages`` for completed non-streaming spans. All are handled.

    Matching: SHA-256 of ``request_data`` plus ``gen_ai.input.messages`` when
    available. Logfire 6 reduced ``request_data`` to the model alone, so matching
    it by itself attached every stream to the oldest call of the same model.
    The bundled input is present on both halves and distinguishes requests.
    Older Logfire shapes without it keep the request-data fallback. Uses FIFO
    queues per key to correctly handle concurrent identical streaming requests.

    Mutation: replaces ``ReadableSpan._context`` and ``._parent``.
    ``ReadableSpan`` has no ``__setattr__`` override, and ``BatchSpanProcessor``
    stores a reference (not copy), so the exporter sees the updated context.

    Ordering: must be added to the TracerProvider BEFORE BatchSpanProcessor
    so mutation completes before the span enters the export queue.
    ``SynchronousMultiSpanProcessor.on_end`` iterates processors in insertion order.

    Applies to all logfire-instrumented providers (OpenAI, Anthropic) and all
    frameworks that delegate to them (OpenAI Agents, PydanticAI).  Non-logfire
    spans are ignored (no ``logfire.span_type`` attribute).
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
