"""SideSeat: AI observability SDK with automatic framework instrumentation."""

from __future__ import annotations

import logging
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any

from sideseat._version import __version__
from sideseat.config import Config, Frameworks
from sideseat.telemetry import TelemetryClient
from sideseat.telemetry.encoding import encode_value, span_to_dict
from sideseat.telemetry.exporters import JsonFileSpanExporter

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
        self._runtime: Any | None = None

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

    def force_flush(self, timeout_millis: int = 30000) -> bool:
        """Force flush pending spans."""
        return self._telemetry.force_flush(timeout_millis)

    def validate_connection(self, timeout: float = 5.0) -> bool:
        """Test connection to SideSeat server."""
        return self._telemetry.validate_connection(timeout)

    def shutdown(self, timeout_millis: int = 30000) -> None:
        """Graceful shutdown with flush."""
        if self._runtime is not None:
            try:
                self._runtime.disconnect()
            except Exception:  # pragma: no cover - best-effort
                logger.debug("runtime disconnect raised", exc_info=True)
        self._telemetry.shutdown(timeout_millis)

    # ------------------------------------------------------------------
    # SDK runtime channel (presence + introspection)
    # ------------------------------------------------------------------

    @property
    def runtime(self) -> Any:
        """Lazy-instantiated runtime client. Requires sideseat[ws] extra."""
        if self._runtime is None:
            from sideseat.runtime.client import RuntimeClient

            self._runtime = RuntimeClient(
                endpoint=self._config.endpoint,
                project_id=self._config.project_id,
            )
        return self._runtime

    def register(
        self,
        objects: Any,
        *,
        name: str | None = None,
        runtime: str | dict[str, Any] = "inproc",
        agentcore_endpoint: str | None = None,
    ) -> SideSeat:
        """Register one object or a list of objects.

        Auto-detects whether each object is an agent or an MCP client via
        the inspector registry, and uses `obj.name` (or the explicit `name=`
        kwarg for a single object) as the registration identity.
        Chainable: `client.register([a]).register([b, mcp]).connect()`.
        """
        self.runtime.register(
            objects,
            name=name,
            runtime=runtime,
            agentcore_endpoint=agentcore_endpoint,
        )
        return self

    def agent(
        self,
        instance: Any,
        *,
        name: str,
        runtime: str | dict[str, Any] = "inproc",
        agentcore_endpoint: str | None = None,
        tools: list[Any] | None = None,
        system_prompt: str | None = None,
        model: str | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> SideSeat:
        """Register an agent for presence + introspection."""
        self.runtime.add_agent(
            instance,
            name=name,
            runtime=runtime,
            agentcore_endpoint=agentcore_endpoint,
            tools=tools,
            system_prompt=system_prompt,
            model=model,
            metadata=metadata,
        )
        return self

    def mcp(
        self,
        client: Any,
        *,
        name: str,
        transport: str | None = None,
        url: str | None = None,
        tools: list[Any] | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> SideSeat:
        """Register an MCP client for presence + introspection."""
        self.runtime.add_mcp(
            client,
            name=name,
            transport=transport,
            url=url,
            tools=tools,
            metadata=metadata,
        )
        return self

    def connect(self, *, block: bool = True, banner: bool = True) -> SideSeat:
        """Open the persistent WebSocket and re-flush the local registry.

        When `block=True` (default), blocks the calling thread until
        `disconnect()` is called or SIGINT/SIGTERM is received. Pass
        `block=False` for embedding scenarios where the caller wants to
        drive its own loop. Pass `banner=False` to suppress the startup
        banner printed on stdout.
        """
        self.runtime.connect(block=block, banner=banner)
        return self

    def disconnect(self) -> None:
        """Send unregisters and close the WebSocket."""
        if self._runtime is not None:
            self._runtime.disconnect()


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


# ---------------------------------------------------------------------------
# Module-level convenience helpers for the WS runtime channel.
# ---------------------------------------------------------------------------


def register(objects: Any, **kwargs: Any) -> SideSeat:
    """Module-level shorthand for `sideseat.get_client().register(...)`."""
    return get_client().register(objects, **kwargs)


def agent(instance: Any, *, name: str, **kwargs: Any) -> SideSeat:
    """Module-level shorthand for `sideseat.get_client().agent(...)`."""
    return get_client().agent(instance, name=name, **kwargs)


def mcp(client: Any, *, name: str, **kwargs: Any) -> SideSeat:
    """Module-level shorthand for `sideseat.get_client().mcp(...)`."""
    return get_client().mcp(client, name=name, **kwargs)


def connect(*, block: bool = True, banner: bool = True) -> SideSeat:
    """Module-level shorthand for `sideseat.get_client().connect(...)`."""
    return get_client().connect(block=block, banner=banner)


def disconnect() -> None:
    """Module-level shorthand for `sideseat.get_client().disconnect()`."""
    get_client().disconnect()


# Wrap logfire.instrument_* early so abstract-method fixes apply to any
# subsequent logfire instrumentation call, whether through SideSeat or not.
try:
    from sideseat.instrumentation import _wrap_logfire_instruments as _wli

    _wli()
    del _wli
except ImportError:
    pass


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
    "register",
    "agent",
    "mcp",
    "connect",
    "disconnect",
]
