"""SideSeat: AI observability SDK with automatic framework instrumentation."""

from __future__ import annotations

import logging
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any

from sideseat._version import __version__
from sideseat.config import Config, Frameworks
from sideseat.instrumentation import patch_adk_tracing, patch_strands_encoder
from sideseat.telemetry import TelemetryClient
from sideseat.telemetry.encoding import encode_value, span_to_dict
from sideseat.telemetry.exporters import JsonFileSpanExporter
from sideseat.telemetry.resource import get_otel_resource

if TYPE_CHECKING:
    from opentelemetry.trace import Span, Tracer

logger = logging.getLogger("sideseat")


class SideSeatError(Exception):
    """Base exception for SideSeat SDK errors."""


_global_instance: SideSeat | None = None
_global_lock = threading.Lock()


class SideSeat:
    """SideSeat observability client.

    Examples:
        # Zero config - auto-detects framework
        SideSeat()

        # Explicit framework
        SideSeat(framework=Frameworks.Strands)

        # With cloud provider instrumentation (direct boto3 usage)
        SideSeat(framework=Frameworks.Bedrock)

        # Framework + provider
        SideSeat(framework=[Frameworks.Strands, Frameworks.Bedrock])

        # Context manager (auto shutdown)
        with SideSeat() as client:
            pass

        # Disabled mode (testing/CI)
        SideSeat(disabled=True)  # or SIDESEAT_DISABLED=true
    """

    def __init__(
        self,
        *,
        disabled: bool | None = None,
        endpoint: str | None = None,
        api_key: str | None = None,
        project_id: str | None = None,
        framework: str | list[str] | None = None,
        service_name: str | None = None,
        service_version: str | None = None,
        auto_instrument: bool = True,
        enable_traces: bool = True,
        enable_metrics: bool = True,
        enable_logs: bool | None = None,
        encode_binary: bool = True,
        capture_content: bool = True,
        debug: bool | None = None,
    ):
        self._config = Config.create(
            disabled=disabled,
            endpoint=endpoint,
            api_key=api_key,
            project_id=project_id,
            framework=framework,
            service_name=service_name,
            service_version=service_version,
            auto_instrument=auto_instrument,
            enable_traces=enable_traces,
            enable_metrics=enable_metrics,
            enable_logs=enable_logs,
            encode_binary=encode_binary,
            capture_content=capture_content,
            debug=debug,
        )
        self._telemetry = TelemetryClient(self._config)

    def __enter__(self) -> SideSeat:
        return self

    def __exit__(self, *args: Any) -> None:
        self.shutdown()

    def __repr__(self) -> str:
        return f"SideSeat(endpoint={self._config.endpoint!r}, project={self._config.project_id!r})"

    @property
    def telemetry(self) -> TelemetryClient:
        """Access telemetry client for additional exporters."""
        return self._telemetry

    @property
    def config(self) -> Config:
        """Access immutable configuration."""
        return self._config

    @property
    def is_disabled(self) -> bool:
        """Check if telemetry is disabled."""
        return self._config.disabled

    @property
    def tracer_provider(self) -> Any:
        """Access tracer provider directly."""
        return self._telemetry.tracer_provider

    def get_tracer(self, name: str = "sideseat") -> Tracer:
        """Get tracer for creating custom spans."""
        return self._telemetry.get_tracer(name)

    @contextmanager
    def span(
        self,
        name: str,
        *,
        user_id: str | None = None,
        session_id: str | None = None,
        **kwargs: Any,
    ) -> Iterator[Span]:
        """Create a span (child of current span if one exists, otherwise root).

        Example:
            with client.span("sub-task") as span:
                do_work()
        """
        with self._telemetry.span(name, user_id=user_id, session_id=session_id, **kwargs) as s:
            yield s

    @contextmanager
    def trace(
        self,
        name: str,
        *,
        user_id: str | None = None,
        session_id: str | None = None,
        **kwargs: Any,
    ) -> Iterator[Span]:
        """Start a trace (root span that groups child spans).

        Example:
            with client.trace("bedrock-converse"):
                bedrock.client.converse(...)
                bedrock.client.converse(...)
        """
        with self._telemetry.span(name, user_id=user_id, session_id=session_id, **kwargs) as s:
            yield s

    @contextmanager
    def session(
        self,
        name: str,
        *,
        session_id: str,
        user_id: str | None = None,
        **kwargs: Any,
    ) -> Iterator[Span]:
        """Start a session trace with an explicit session ID.

        Example:
            with client.session("chat", session_id="sess-123"):
                bedrock.client.converse(...)
        """
        with self._telemetry.span(name, user_id=user_id, session_id=session_id, **kwargs) as s:
            yield s

    def force_flush(self, timeout_millis: int = 30000) -> bool:
        """Force flush pending spans."""
        return self._telemetry.force_flush(timeout_millis)

    def validate_connection(self, timeout: float = 5.0) -> bool:
        """Test connection to SideSeat server."""
        return self._telemetry.validate_connection(timeout)

    def shutdown(self, timeout_millis: int = 30000) -> None:
        """Graceful shutdown with flush."""
        self._telemetry.shutdown(timeout_millis)


def init(**kwargs: Any) -> SideSeat:
    """Initialize global SideSeat instance (thread-safe)."""
    global _global_instance
    with _global_lock:
        if _global_instance is not None:
            logger.warning("SideSeat already initialized; returning existing instance")
            return _global_instance
        _global_instance = SideSeat(**kwargs)
        return _global_instance


def get_client() -> SideSeat:
    """Get global SideSeat instance. Raises if not initialized."""
    with _global_lock:
        if _global_instance is None:
            raise SideSeatError("SideSeat not initialized. Call sideseat.init() first.")
        return _global_instance


def shutdown() -> None:
    """Shutdown global SideSeat instance (for cleanup)."""
    global _global_instance
    with _global_lock:
        if _global_instance is not None:
            _global_instance.shutdown()
            _global_instance = None


def is_initialized() -> bool:
    """Check if global SideSeat instance exists."""
    with _global_lock:
        return _global_instance is not None


__all__ = [
    "__version__",
    "SideSeat",
    "Frameworks",
    "SideSeatError",
    "TelemetryClient",
    "JsonFileSpanExporter",
    "Config",
    "init",
    "get_client",
    "shutdown",
    "is_initialized",
    "encode_value",
    "span_to_dict",
    "patch_adk_tracing",
    "patch_strands_encoder",
    "get_otel_resource",
]
