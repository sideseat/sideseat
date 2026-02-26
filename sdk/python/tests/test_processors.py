"""Tests for logfire streaming span processor."""

from __future__ import annotations

import time
from unittest.mock import MagicMock

from opentelemetry.trace import SpanContext, TraceFlags

from sideseat.telemetry.processors import _LogfireStreamingProcessor


def _make_span(
    trace_id: int,
    span_id: int,
    attrs: dict[str, str],
    parent: SpanContext | None = None,
) -> MagicMock:
    """Create a mock span with the given context and attributes."""
    span = MagicMock()
    span.context = SpanContext(
        trace_id=trace_id,
        span_id=span_id,
        is_remote=False,
        trace_flags=TraceFlags(TraceFlags.SAMPLED),
    )
    span._context = span.context
    span._parent = parent
    span.attributes = attrs
    return span


def _entry_count(proc: _LogfireStreamingProcessor) -> int:
    """Total entries across all keys."""
    return sum(len(v) for v in proc._pending.values())


REQUEST_DATA = (
    '{"messages": [{"role": "user", "content": "Hi"}], "model": "gpt-4o", "stream": true}'
)


class TestLogfireStreamingProcessor:
    def test_reparents_response_log(self) -> None:
        """Response log should get the request span's trace_id and parent."""
        proc = _LogfireStreamingProcessor()

        request_span = _make_span(
            trace_id=0xAABB,
            span_id=0x1111,
            attrs={
                "logfire.span_type": "span",
                "request_data": REQUEST_DATA,
            },
        )
        proc.on_end(request_span)

        response_log = _make_span(
            trace_id=0xCCDD,
            span_id=0x2222,
            attrs={
                "logfire.span_type": "log",
                "request_data": REQUEST_DATA,
                "response_data": '{"message": {"role": "assistant", "content": "Hello"}}',
            },
        )
        proc.on_end(response_log)

        assert response_log._context.trace_id == 0xAABB
        assert response_log._context.span_id == 0x2222  # own span_id preserved
        assert response_log._parent.trace_id == 0xAABB
        assert response_log._parent.span_id == 0x1111

    def test_reparents_responses_api_log(self) -> None:
        """Responses API streaming log (no response_data, has events) should also be reparented."""
        proc = _LogfireStreamingProcessor()

        request_span = _make_span(
            trace_id=0xAABB,
            span_id=0x1111,
            attrs={
                "logfire.span_type": "span",
                "request_data": '{"model":"gpt-5-nano","stream":true}',
            },
        )
        proc.on_end(request_span)

        response_log = _make_span(
            trace_id=0xCCDD,
            span_id=0x2222,
            attrs={
                "logfire.span_type": "log",
                "request_data": '{"model":"gpt-5-nano","stream":true}',
                "events": '[{"event.name":"gen_ai.user.message","content":"Hi","role":"user"}]',
            },
        )
        proc.on_end(response_log)

        assert response_log._context.trace_id == 0xAABB
        assert response_log._context.span_id == 0x2222
        assert response_log._parent.trace_id == 0xAABB
        assert response_log._parent.span_id == 0x1111

    def test_non_streaming_span_ignored(self) -> None:
        """Span with both request_data and response_data (non-streaming) is not stored."""
        proc = _LogfireStreamingProcessor()

        non_streaming = _make_span(
            trace_id=0xAABB,
            span_id=0x1111,
            attrs={
                "logfire.span_type": "span",
                "request_data": REQUEST_DATA,
                "response_data": '{"message": {"role": "assistant"}}',
            },
        )
        proc.on_end(non_streaming)
        assert _entry_count(proc) == 0

    def test_unmatched_response_log_unchanged(self) -> None:
        """Response log without matching request span is not modified."""
        proc = _LogfireStreamingProcessor()

        response_log = _make_span(
            trace_id=0xCCDD,
            span_id=0x2222,
            attrs={
                "logfire.span_type": "log",
                "request_data": REQUEST_DATA,
                "response_data": '{"message": {"role": "assistant"}}',
            },
        )
        proc.on_end(response_log)

        assert response_log._context.trace_id == 0xCCDD

    def test_same_trace_id_skipped(self) -> None:
        """If trace_ids already match (parent span active), no modification."""
        proc = _LogfireStreamingProcessor()

        request_span = _make_span(
            trace_id=0xAABB,
            span_id=0x1111,
            attrs={
                "logfire.span_type": "span",
                "request_data": REQUEST_DATA,
            },
        )
        proc.on_end(request_span)

        response_log = _make_span(
            trace_id=0xAABB,
            span_id=0x2222,
            attrs={
                "logfire.span_type": "log",
                "request_data": REQUEST_DATA,
                "response_data": '{"message": {"role": "assistant"}}',
            },
        )
        original_context = response_log._context
        proc.on_end(response_log)

        assert response_log._context is original_context
        assert _entry_count(proc) == 0  # entry consumed even though span unchanged

    def test_pending_entry_consumed(self) -> None:
        """Matching response log should consume (remove) the pending entry."""
        proc = _LogfireStreamingProcessor()

        request_span = _make_span(
            trace_id=0xAABB,
            span_id=0x1111,
            attrs={
                "logfire.span_type": "span",
                "request_data": REQUEST_DATA,
            },
        )
        proc.on_end(request_span)
        assert _entry_count(proc) == 1

        response_log = _make_span(
            trace_id=0xCCDD,
            span_id=0x2222,
            attrs={
                "logfire.span_type": "log",
                "request_data": REQUEST_DATA,
                "response_data": '{"message": {}}',
            },
        )
        proc.on_end(response_log)
        assert _entry_count(proc) == 0

    def test_unrelated_spans_ignored(self) -> None:
        """Non-logfire spans are not affected."""
        proc = _LogfireStreamingProcessor()

        span = _make_span(
            trace_id=0xAABB,
            span_id=0x1111,
            attrs={"gen_ai.system": "openai"},
        )
        proc.on_end(span)
        assert _entry_count(proc) == 0

    def test_no_attributes_ignored(self) -> None:
        """Span with no attributes is safely ignored."""
        proc = _LogfireStreamingProcessor()

        span = MagicMock()
        span.attributes = None
        proc.on_end(span)
        assert _entry_count(proc) == 0

    def test_non_string_span_type_ignored(self) -> None:
        """Non-string logfire.span_type is safely ignored."""
        proc = _LogfireStreamingProcessor()

        span = _make_span(
            trace_id=0xAABB,
            span_id=0x1111,
            attrs={"logfire.span_type": 42, "request_data": REQUEST_DATA},  # type: ignore[dict-item]
        )
        proc.on_end(span)
        assert _entry_count(proc) == 0

    def test_non_string_request_data_ignored(self) -> None:
        """Non-string request_data is safely ignored."""
        proc = _LogfireStreamingProcessor()

        span = _make_span(
            trace_id=0xAABB,
            span_id=0x1111,
            attrs={"logfire.span_type": "span", "request_data": 123},  # type: ignore[dict-item]
        )
        proc.on_end(span)
        assert _entry_count(proc) == 0

    def test_mutation_failure_restores_pending(self) -> None:
        """If span mutation fails, pending entry is restored for retry."""
        proc = _LogfireStreamingProcessor()

        request_span = _make_span(
            trace_id=0xAABB,
            span_id=0x1111,
            attrs={
                "logfire.span_type": "span",
                "request_data": REQUEST_DATA,
            },
        )
        proc.on_end(request_span)

        # Create a span that will fail on _context assignment
        response_log = _make_span(
            trace_id=0xCCDD,
            span_id=0x2222,
            attrs={
                "logfire.span_type": "log",
                "request_data": REQUEST_DATA,
                "response_data": '{"message": {}}',
            },
        )
        # Make _context assignment raise
        type(response_log)._context = property(
            lambda self: SpanContext(
                trace_id=0xCCDD,
                span_id=0x2222,
                is_remote=False,
                trace_flags=TraceFlags(TraceFlags.SAMPLED),
            ),
            lambda self, v: (_ for _ in ()).throw(RuntimeError("frozen")),
        )
        proc.on_end(response_log)

        # Pending entry should be restored
        assert _entry_count(proc) == 1

    def test_ttl_cleanup(self) -> None:
        """Stale entries should be cleaned up."""
        proc = _LogfireStreamingProcessor()

        request_span = _make_span(
            trace_id=0xAABB,
            span_id=0x1111,
            attrs={
                "logfire.span_type": "span",
                "request_data": '{"messages": [{"role": "user", "content": "old"}], "stream": 1}',
            },
        )
        proc.on_end(request_span)
        assert _entry_count(proc) == 1

        # Manually backdate the entry to make it stale
        key = list(proc._pending.keys())[0]
        trace_id, span_id, _ = proc._pending[key][0]
        proc._pending[key] = [(trace_id, span_id, time.monotonic() - 120)]

        request_span2 = _make_span(
            trace_id=0xEEFF,
            span_id=0x3333,
            attrs={
                "logfire.span_type": "span",
                "request_data": REQUEST_DATA,
            },
        )
        proc.on_end(request_span2)
        assert _entry_count(proc) == 1

    def test_shutdown_clears_pending(self) -> None:
        """Shutdown should clear all pending entries."""
        proc = _LogfireStreamingProcessor()

        request_span = _make_span(
            trace_id=0xAABB,
            span_id=0x1111,
            attrs={
                "logfire.span_type": "span",
                "request_data": REQUEST_DATA,
            },
        )
        proc.on_end(request_span)
        assert _entry_count(proc) == 1

        proc.shutdown()
        assert _entry_count(proc) == 0

    def test_concurrent_identical_requests(self) -> None:
        """Two concurrent streaming requests with identical request_data both get matched."""
        proc = _LogfireStreamingProcessor()

        # Two request spans with identical request_data but different traces
        req_a = _make_span(
            trace_id=0xAAAA,
            span_id=0x1111,
            attrs={"logfire.span_type": "span", "request_data": REQUEST_DATA},
        )
        req_b = _make_span(
            trace_id=0xBBBB,
            span_id=0x2222,
            attrs={"logfire.span_type": "span", "request_data": REQUEST_DATA},
        )
        proc.on_end(req_a)
        proc.on_end(req_b)
        assert _entry_count(proc) == 2
        assert len(proc._pending) == 1  # same key, two entries in list

        # First response matches first request (FIFO)
        resp_a = _make_span(
            trace_id=0xCC01,
            span_id=0x3333,
            attrs={
                "logfire.span_type": "log",
                "request_data": REQUEST_DATA,
                "response_data": '{"message": {"role": "assistant", "content": "A"}}',
            },
        )
        proc.on_end(resp_a)
        assert resp_a._context.trace_id == 0xAAAA
        assert resp_a._parent.span_id == 0x1111
        assert _entry_count(proc) == 1

        # Second response matches second request
        resp_b = _make_span(
            trace_id=0xCC02,
            span_id=0x4444,
            attrs={
                "logfire.span_type": "log",
                "request_data": REQUEST_DATA,
                "response_data": '{"message": {"role": "assistant", "content": "B"}}',
            },
        )
        proc.on_end(resp_b)
        assert resp_b._context.trace_id == 0xBBBB
        assert resp_b._parent.span_id == 0x2222
        assert _entry_count(proc) == 0

    def test_max_pending_eviction(self) -> None:
        """Entries beyond _MAX_PENDING are evicted oldest-first."""
        proc = _LogfireStreamingProcessor()
        proc._MAX_PENDING = 5  # type: ignore[assignment]

        # Add 7 entries (each with unique request_data)
        for i in range(7):
            span = _make_span(
                trace_id=0x1000 + i,
                span_id=0x2000 + i,
                attrs={
                    "logfire.span_type": "span",
                    "request_data": f'{{"messages": [], "model": "m", "id": {i}, "stream": true}}',
                },
            )
            proc.on_end(span)

        assert _entry_count(proc) == 5
        # Oldest entries (id=0, id=1) should have been evicted
        remaining_trace_ids = {entries[0][0] for entries in proc._pending.values()}
        assert 0x1000 not in remaining_trace_ids  # id=0 evicted
        assert 0x1001 not in remaining_trace_ids  # id=1 evicted

    def test_different_request_data_independent(self) -> None:
        """Requests with different request_data do not interfere."""
        proc = _LogfireStreamingProcessor()

        req_data_1 = (
            '{"messages": [{"role": "user", "content": "A"}], "model": "gpt-4o", "stream": true}'
        )
        req_data_2 = (
            '{"messages": [{"role": "user", "content": "B"}], "model": "gpt-4o", "stream": true}'
        )

        req1 = _make_span(
            trace_id=0xAAAA,
            span_id=0x1111,
            attrs={"logfire.span_type": "span", "request_data": req_data_1},
        )
        req2 = _make_span(
            trace_id=0xBBBB,
            span_id=0x2222,
            attrs={"logfire.span_type": "span", "request_data": req_data_2},
        )
        proc.on_end(req1)
        proc.on_end(req2)
        assert _entry_count(proc) == 2
        assert len(proc._pending) == 2  # different keys

        # Response for req2 arrives first — should match req2, not req1
        resp2 = _make_span(
            trace_id=0xDD02,
            span_id=0x4444,
            attrs={
                "logfire.span_type": "log",
                "request_data": req_data_2,
                "response_data": '{"message": {"role": "assistant"}}',
            },
        )
        proc.on_end(resp2)
        assert resp2._context.trace_id == 0xBBBB
        assert _entry_count(proc) == 1

        # Response for req1 arrives second
        resp1 = _make_span(
            trace_id=0xDD01,
            span_id=0x3333,
            attrs={
                "logfire.span_type": "log",
                "request_data": req_data_1,
                "response_data": '{"message": {"role": "assistant"}}',
            },
        )
        proc.on_end(resp1)
        assert resp1._context.trace_id == 0xAAAA
        assert _entry_count(proc) == 0
