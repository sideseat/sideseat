"""Session and user correlation carried in the OpenTelemetry context.

The values live under a private context key rather than in W3C baggage. Baggage is injected into
every outgoing request by HTTP client instrumentation, which would send end-user identifiers to
model providers and other third parties. A private key follows the same paths inside the process -
async tasks, and threads that propagate the OpenTelemetry context - and never leaves it.
"""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass, replace
from typing import Any

from opentelemetry import context as otel_context
from opentelemetry.sdk.trace import ReadableSpan, Span, SpanProcessor

SESSION_ID = "session.id"
USER_ID = "user.id"

_KEY = otel_context.create_key("sideseat.correlation")


@dataclass(frozen=True, slots=True)
class Correlation:
    """The session and user a span belongs to."""

    session_id: str | None = None
    user_id: str | None = None


def current(ctx: otel_context.Context | None = None) -> Correlation:
    """The correlation in effect for ``ctx``, or for the current context."""
    value = otel_context.get_value(_KEY, ctx)
    return value if isinstance(value, Correlation) else Correlation()


def with_correlation(
    ctx: otel_context.Context, *, session_id: str | None, user_id: str | None
) -> otel_context.Context:
    """``ctx`` with the given values layered over the correlation it already carries."""
    merged = current(ctx)
    if session_id is not None:
        merged = replace(merged, session_id=_require_text("session_id", session_id))
    if user_id is not None:
        merged = replace(merged, user_id=_require_text("user_id", user_id))
    return otel_context.set_value(_KEY, merged, ctx)


@contextmanager
def scope(*, session_id: str | None, user_id: str | None) -> Iterator[Correlation]:
    token = otel_context.attach(
        with_correlation(otel_context.get_current(), session_id=session_id, user_id=user_id)
    )
    try:
        yield current()
    finally:
        otel_context.detach(token)


class CorrelationProcessor(SpanProcessor):
    """Stamps ``session.id`` and ``user.id`` on every span started inside a correlation scope.

    Registered first on the tracer provider so the attributes exist before any other processor or
    exporter reads the span. A scope wins over a value the producer set at span creation: the
    application said which session this work belongs to.
    """

    def on_start(self, span: Span, parent_context: otel_context.Context | None = None) -> None:
        correlation = current(parent_context)
        if correlation.session_id is not None:
            span.set_attribute(SESSION_ID, correlation.session_id)
        if correlation.user_id is not None:
            span.set_attribute(USER_ID, correlation.user_id)

    def on_end(self, span: ReadableSpan) -> None:
        pass

    def shutdown(self) -> None:
        pass

    def force_flush(self, timeout_millis: int = 30000) -> bool:
        return True


def _require_text(name: str, value: Any) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{name} must be a non-empty string, got {value!r}")
    return value
