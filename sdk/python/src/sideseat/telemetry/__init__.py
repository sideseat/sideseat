"""TelemetryClient: TracerProvider setup and span export."""

from __future__ import annotations

import atexit
import contextvars
import logging
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any
from urllib.parse import urlparse

from opentelemetry import trace
from opentelemetry.trace import StatusCode

if TYPE_CHECKING:
    from opentelemetry.trace import Span, Tracer

    from sideseat.config import Config

logger = logging.getLogger("sideseat.telemetry")

_user_id_var: contextvars.ContextVar[str | None] = contextvars.ContextVar(
    "sideseat_user_id", default=None
)
_session_id_var: contextvars.ContextVar[str | None] = contextvars.ContextVar(
    "sideseat_session_id", default=None
)


class _ContextSpanProcessor:
    """Injects session.id and user.id into every span on creation."""

    def __init__(self, user_id: str | None, session_id: str | None) -> None:
        self._user_id = user_id
        self._session_id = session_id

    def on_start(self, span: Any, parent_context: Any = None) -> None:
        uid = _user_id_var.get() or self._user_id
        sid = _session_id_var.get() or self._session_id
        if uid:
            span.set_attribute("user.id", uid)
        if sid:
            span.set_attribute("session.id", sid)

    def on_end(self, span: Any) -> None:
        pass

    def shutdown(self) -> None:
        pass

    def force_flush(self, timeout_millis: int = 30000) -> bool:
        return True


class TelemetryClient:
    """Manages TracerProvider and span export."""

    tracer_provider: Any  # Can be SDK TracerProvider or NoOp

    def __init__(self, config: Config) -> None:
        self._config = config
        self._file_exporters: list[Any] = []
        self._shutdown_lock = threading.Lock()
        self._shutdown_called = False
        self._atexit_registered = False
        self._otlp_processor: Any = None
        self._disabled = config.disabled

        # Skip all setup if disabled
        if config.disabled:
            logger.info("SideSeat disabled - no telemetry will be collected")
            self.tracer_provider = trace.get_tracer_provider()  # NoOp provider
            return

        from sideseat.instrumentation import (
            apply_framework_patches,
            instrument,
            is_logfire_framework,
        )

        # Debug logging
        if config.debug:
            logging.getLogger("sideseat").setLevel(logging.DEBUG)
            logging.getLogger("opentelemetry").setLevel(logging.DEBUG)
            logger.debug("SideSeat debug mode enabled")
            logger.debug(
                "Config: endpoint=%s project=%s framework=%s",
                config.endpoint,
                config.project_id,
                config.framework,
            )

        # Framework-specific patches (binary encoding, ADK tracing)
        apply_framework_patches(config.framework, config.encode_binary)

        # Provider initialization
        logfire_mode = is_logfire_framework(config.framework) and config.auto_instrument
        if logfire_mode:
            self._init_logfire_mode(instrument)
        else:
            self._init_standard_mode(instrument)

        self.tracer_provider.add_span_processor(
            _ContextSpanProcessor(config.user_id, config.session_id)
        )

        # Auto-setup metrics and logs (isolated - failures don't crash SDK)
        # Skip for logfire frameworks: logfire.configure() manages these providers
        if not logfire_mode:
            if config.enable_metrics:
                try:
                    from sideseat.telemetry.setup import setup_metrics

                    self.meter_provider = setup_metrics(self._config)
                except Exception as e:
                    logger.warning("Failed to setup metrics: %s", e)
            if config.enable_logs:
                try:
                    from sideseat.telemetry.setup import setup_logs

                    self.logger_provider, self._logging_handler = setup_logs(self._config)
                except Exception as e:
                    logger.warning("Failed to setup logs: %s", e)

    def _init_standard_mode(self, instrument_fn: Any) -> None:
        """Standard mode: we own the TracerProvider."""
        from opentelemetry.sdk.trace import TracerProvider as SDKTracerProvider

        from sideseat.telemetry.resource import get_otel_resource
        from sideseat.telemetry.setup import setup_otlp, setup_propagators

        # Check for existing provider
        existing = trace.get_tracer_provider()
        if isinstance(existing, SDKTracerProvider):
            logger.warning("TracerProvider already set; adding to existing")
            self.tracer_provider = existing
        else:
            resource = get_otel_resource(self._config.service_name, self._config.service_version)
            self.tracer_provider = SDKTracerProvider(resource=resource)
            trace.set_tracer_provider(self.tracer_provider)
            setup_propagators()

        self._otlp_processor = setup_otlp(self._config, self.tracer_provider)

        if self._config.auto_instrument:
            instrument_fn(
                self._config.framework,
                self.tracer_provider,
                self._config.service_name,
                self._config.service_version,
            )

        # Instrument cloud providers if explicitly requested
        from sideseat.instrumentation import instrument_providers

        instrument_providers(self.tracer_provider, self._config.providers)

    def _init_logfire_mode(self, instrument_fn: Any) -> None:
        """Logfire mode: logfire owns provider, we add OTLP."""
        from sideseat.telemetry.setup import setup_otlp

        instrument_fn(
            self._config.framework,
            None,
            self._config.service_name,
            self._config.service_version,
        )

        self.tracer_provider = trace.get_tracer_provider()
        if not hasattr(self.tracer_provider, "add_span_processor"):
            raise RuntimeError("Logfire did not create valid TracerProvider")
        self._otlp_processor = setup_otlp(self._config, self.tracer_provider)

        # Instrument cloud providers if explicitly requested
        from sideseat.instrumentation import instrument_providers

        instrument_providers(self.tracer_provider, self._config.providers)

    def _build_endpoint(self, signal: str) -> str:
        """Build endpoint URL for a signal (traces, metrics, logs)."""
        from sideseat.telemetry.setup import build_endpoint

        return build_endpoint(self._config, signal)

    def get_tracer(self, name: str = "sideseat", version: str | None = None) -> Tracer:
        """Get tracer for custom spans."""
        from sideseat._version import __version__

        tracer: Tracer = self.tracer_provider.get_tracer(name, version or __version__)
        return tracer

    @contextmanager
    def span(self, name: str, **kwargs: Any) -> Iterator[Span]:
        """Context manager for spans with auto error status."""
        user_id = kwargs.pop("user_id", None)
        session_id = kwargs.pop("session_id", None)
        tokens: list[Any] = []
        if user_id is not None:
            tokens.append(_user_id_var.set(user_id))
        if session_id is not None:
            tokens.append(_session_id_var.set(session_id))
        try:
            with self.get_tracer().start_as_current_span(name, **kwargs) as s:
                try:
                    yield s
                except Exception as e:
                    s.set_status(StatusCode.ERROR, str(e))
                    s.record_exception(e)
                    raise
        finally:
            for token in tokens:
                token.var.reset(token)

    def setup_console_exporter(self, **kwargs: Any) -> TelemetryClient:
        """Add console exporter for debugging."""
        if self._disabled:
            return self
        from opentelemetry.sdk.trace.export import ConsoleSpanExporter, SimpleSpanProcessor

        self.tracer_provider.add_span_processor(SimpleSpanProcessor(ConsoleSpanExporter(**kwargs)))
        return self

    def setup_file_exporter(self, path: str = "traces.jsonl", mode: str = "a") -> TelemetryClient:
        """Add JSONL file exporter."""
        if self._disabled:
            return self
        from opentelemetry.sdk.trace.export import BatchSpanProcessor

        from sideseat.telemetry.exporters import JsonFileSpanExporter

        exp = JsonFileSpanExporter(path, mode)
        self.tracer_provider.add_span_processor(BatchSpanProcessor(exp))
        self._file_exporters.append(exp)
        self._register_atexit()
        return self

    def validate_connection(self, timeout: float = 5.0) -> bool:
        """Test connection to SideSeat server. Returns False if disabled."""
        if self._disabled:
            return False
        import urllib.request

        try:
            # Build health URL from base (scheme + host + port), ignoring any path
            parsed = urlparse(self._config.endpoint)
            base_url = f"{parsed.scheme}://{parsed.netloc}"
            url = f"{base_url}/api/v1/health"
            with urllib.request.urlopen(url, timeout=timeout) as r:
                return r.status == 200  # type: ignore[no-any-return]
        except Exception as e:
            logger.debug("Connection check failed: %s", e)
            return False

    def force_flush(self, timeout_millis: int = 30000) -> bool:
        """Force flush all pending spans."""
        if self._otlp_processor:
            return self._otlp_processor.force_flush(timeout_millis)
        return True

    def _register_atexit(self) -> None:
        if not self._atexit_registered:
            atexit.register(self.shutdown)
            self._atexit_registered = True

    def shutdown(self, timeout_millis: int = 30000) -> None:
        """Graceful shutdown with flush."""
        with self._shutdown_lock:
            if self._shutdown_called:
                return
            self._shutdown_called = True

        # Flush before shutdown
        if self._otlp_processor:
            try:
                self._otlp_processor.force_flush(timeout_millis)
            except Exception as e:
                logger.warning("OTLP flush failed: %s", e)

        for exp in self._file_exporters:
            exp.shutdown()

        if hasattr(self.tracer_provider, "shutdown"):
            self.tracer_provider.shutdown()
        if hasattr(self, "meter_provider"):
            self.meter_provider.shutdown()
        if hasattr(self, "logger_provider"):
            # Remove logging handler before shutdown to prevent leakage
            if hasattr(self, "_logging_handler"):
                import logging as stdlib_logging

                stdlib_logging.getLogger().removeHandler(self._logging_handler)
            self.logger_provider.shutdown()
