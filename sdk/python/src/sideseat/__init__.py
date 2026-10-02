"""SideSeat: OpenTelemetry for AI agents, configured in one call.

::

    import sideseat

    sideseat.init(integrations=["strands"])

    with sideseat.session("conversation-42", user_id="user-7"):
        agent("Plan a trip to Kyoto")

See https://sideseat.ai/docs/sdks/python/ for every option.
"""

from __future__ import annotations

import threading
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager
from typing import Any, TypeVar

from opentelemetry.trace import Span, SpanKind

from sideseat import _config
from sideseat._client import SideSeat
from sideseat._version import __version__
from sideseat.errors import ConfigurationError, IntegrationError, SideSeatError
from sideseat.integrations import Integration

F = TypeVar("F", bound=Callable[..., Any])

_lock = threading.Lock()
_client: SideSeat | None = None


def init(
    *,
    endpoint: str | None = None,
    project: str | None = None,
    api_key: str | None = None,
    service_name: str | None = None,
    service_version: str | None = None,
    integrations: Sequence[str | Integration] | str | None = None,
    capture_content: bool | None = None,
    disabled: bool | None = None,
    debug: bool | None = None,
    export: bool = True,
    metrics: bool = True,
    logs: bool = True,
    capture_python_logs: bool = False,
    resource_attributes: Mapping[str, Any] | None = None,
    span_processors: Sequence[Any] | None = None,
) -> SideSeat:
    """Configure telemetry for this process and return the client.

    Args:
        endpoint: SideSeat server URL, or an OTLP base URL that already has a path.
        project: Project that receives the telemetry.
        api_key: Sent as a bearer token.
        service_name: ``service.name``; defaults to the primary integration's package name.
        service_version: ``service.version``; defaults to that package's version.
        integrations: Names or :class:`~sideseat.integrations.Integration` instances. The first is
            the primary integration. ``None`` detects the installed framework.
        capture_content: Record prompts, responses, and tool payloads. On by default.
        disabled: Configure nothing; every call becomes a no-op.
        debug: Log the SDK's decisions at debug level.
        export: Send telemetry over OTLP. Off is useful with ``span_processors`` in tests.
        metrics: Export OpenTelemetry metrics.
        logs: Export OpenTelemetry log records, which some instrumentations use for GenAI events.
        capture_python_logs: Also export records from Python's ``logging`` root logger.
        resource_attributes: Extra resource attributes for every signal.
        span_processors: Extra processors, run after correlation and before export.

    Calling ``init`` again with the same arguments returns the same client. Different arguments
    raise :class:`ConfigurationError`: telemetry configuration is process-wide.
    """
    global _client
    settings = _config.resolve(
        endpoint=endpoint,
        project=project,
        api_key=api_key,
        service_name=service_name,
        service_version=service_version,
        integrations=integrations,
        capture_content=capture_content,
        disabled=disabled,
        debug=debug,
        export=export,
        metrics=metrics,
        logs=logs,
        capture_python_logs=capture_python_logs,
        resource_attributes=resource_attributes,
        span_processors=span_processors,
    )
    with _lock:
        if _client is not None:
            if _client.settings.identity() != settings.identity():
                raise ConfigurationError(
                    "sideseat.init was already called with different settings; "
                    "call sideseat.shutdown() first"
                )
            return _client
        _client = SideSeat(settings)
        return _client


def get_client() -> SideSeat:
    """The client :func:`init` created."""
    with _lock:
        if _client is None:
            raise SideSeatError("call sideseat.init() first")
        return _client


def flush(timeout_millis: int = 30_000) -> bool:
    """Export everything pending. Returns whether all of it was exported."""
    return get_client().flush(timeout_millis)


def shutdown(timeout_millis: int = 30_000) -> bool:
    """Flush and stop the pipeline. Runs automatically at exit; safe to call more than once."""
    global _client
    with _lock:
        client, _client = _client, None
    return True if client is None else client.shutdown(timeout_millis)


@contextmanager
def session(session_id: str, *, user_id: str | None = None) -> Iterator[None]:
    """Attribute every span started inside the block to a session and, optionally, a user."""
    with get_client().session(session_id, user_id=user_id):
        yield


@contextmanager
def trace(
    name: str,
    *,
    session_id: str | None = None,
    user_id: str | None = None,
    attributes: Mapping[str, Any] | None = None,
    kind: SpanKind = SpanKind.INTERNAL,
) -> Iterator[Span]:
    """Start a new trace: a root span, even inside another span."""
    with get_client().trace(
        name, session_id=session_id, user_id=user_id, attributes=attributes, kind=kind
    ) as span:
        yield span


@contextmanager
def span(
    name: str,
    *,
    attributes: Mapping[str, Any] | None = None,
    kind: SpanKind = SpanKind.INTERNAL,
) -> Iterator[Span]:
    """Start a child of the active span."""
    with get_client().span(name, attributes=attributes, kind=kind) as s:
        yield s


def observe(
    name: str | None = None,
    *,
    attributes: Mapping[str, Any] | None = None,
    kind: SpanKind = SpanKind.INTERNAL,
) -> Callable[[F], F]:
    """Run each call of the decorated function in a span named after it.

    The client is looked up at call time, so the decorator can be applied before :func:`init`.
    """

    def decorate(func: F) -> F:
        import functools
        import inspect

        if inspect.iscoroutinefunction(func):

            @functools.wraps(func)
            async def run_async(*args: Any, **kwargs: Any) -> Any:
                wrapped = get_client().observe(name, attributes=attributes, kind=kind)(func)
                return await wrapped(*args, **kwargs)

            return run_async  # type: ignore[return-value]

        @functools.wraps(func)
        def run(*args: Any, **kwargs: Any) -> Any:
            return get_client().observe(name, attributes=attributes, kind=kind)(func)(
                *args, **kwargs
            )

        return run  # type: ignore[return-value]

    return decorate


__all__ = [
    "ConfigurationError",
    "Integration",
    "IntegrationError",
    "SideSeat",
    "SideSeatError",
    "__version__",
    "flush",
    "get_client",
    "init",
    "observe",
    "session",
    "shutdown",
    "span",
    "trace",
]
