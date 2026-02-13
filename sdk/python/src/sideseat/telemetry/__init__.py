"""TelemetryClient: TracerProvider setup and span export."""

from __future__ import annotations

import atexit
import logging
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from typing import TYPE_CHECKING, Any
from urllib.parse import urlparse

from opentelemetry import propagate, trace
from opentelemetry.baggage.propagation import W3CBaggagePropagator
from opentelemetry.propagators.composite import CompositePropagator
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import (
    BatchSpanProcessor,
    ConsoleSpanExporter,
    SimpleSpanProcessor,
)
from opentelemetry.trace import Status, StatusCode
from opentelemetry.trace.propagation.tracecontext import TraceContextTextMapPropagator

if TYPE_CHECKING:
    from opentelemetry.trace import Span, Tracer

    from sideseat.config import Config

from sideseat.config import Frameworks

logger = logging.getLogger("sideseat.telemetry")


class TelemetryClient:
    """Manages TracerProvider and span export."""

    tracer_provider: Any  # Can be SDK TracerProvider or NoOp

    def __init__(self, config: Config) -> None:
        self._config = config
        self._file_exporters: list[Any] = []
        self._shutdown_lock = threading.Lock()
        self._shutdown_called = False
        self._atexit_registered = False
        self._otlp_processor: BatchSpanProcessor | None = None
        self._disabled = config.disabled

        # Skip all setup if disabled
        if config.disabled:
            logger.info("SideSeat disabled - no telemetry will be collected")
            self.tracer_provider = trace.get_tracer_provider()  # NoOp provider
            return

        from sideseat.instrumentation import instrument, is_logfire_framework
        from sideseat.telemetry.encoding import patch_adk_tracing, patch_strands_encoder

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

        # Binary encoding for Strands (patch early, before any encoder use)
        if config.encode_binary and config.framework == Frameworks.Strands:
            patch_strands_encoder()

        # Preserve inline_data metadata in ADK telemetry
        if config.framework == Frameworks.GoogleADK:
            patch_adk_tracing()

        # Provider initialization
        logfire_mode = is_logfire_framework(config.framework) and config.auto_instrument
        if logfire_mode:
            self._init_logfire_mode(instrument)
        else:
            self._init_standard_mode(instrument)

        # Auto-setup metrics and logs (isolated - failures don't crash SDK)
        # Skip for logfire frameworks: logfire.configure() manages these providers
        if not logfire_mode:
            if config.enable_metrics:
                try:
                    self._setup_metrics()
                except Exception as e:
                    logger.warning("Failed to setup metrics: %s", e)
            if config.enable_logs:
                try:
                    self._setup_logs()
                except Exception as e:
                    logger.warning("Failed to setup logs: %s", e)

    def _init_standard_mode(self, instrument_fn: Any) -> None:
        """Standard mode: we own the TracerProvider."""
        from opentelemetry.sdk.trace import TracerProvider as SDKTracerProvider

        from sideseat.telemetry.encoding import get_otel_resource

        # Check for existing provider
        existing = trace.get_tracer_provider()
        if isinstance(existing, SDKTracerProvider):
            logger.warning("TracerProvider already set; adding to existing")
            self.tracer_provider = existing
        else:
            resource = get_otel_resource(self._config.service_name, self._config.service_version)
            self.tracer_provider = TracerProvider(resource=resource)
            trace.set_tracer_provider(self.tracer_provider)
            self._setup_propagators()

        self._setup_otlp()

        if self._config.auto_instrument:
            instrument_fn(
                self._config.framework,
                self.tracer_provider,
                self._config.service_name,
                self._config.service_version,
            )

    def _init_logfire_mode(self, instrument_fn: Any) -> None:
        """Logfire mode: logfire owns provider, we add OTLP."""
        instrument_fn(
            self._config.framework,
            None,
            self._config.service_name,
            self._config.service_version,
        )

        self.tracer_provider = trace.get_tracer_provider()
        if not hasattr(self.tracer_provider, "add_span_processor"):
            raise RuntimeError("Logfire did not create valid TracerProvider")
        self._setup_otlp()

    def _setup_propagators(self) -> None:
        propagate.set_global_textmap(
            CompositePropagator(
                [
                    W3CBaggagePropagator(),
                    TraceContextTextMapPropagator(),
                ]
            )
        )

    def _setup_otlp(self) -> None:
        """Set up OTLP trace export."""
        if not self._config.enable_traces:
            return
        from opentelemetry.exporter.otlp.proto.http.trace_exporter import (
            OTLPSpanExporter,
        )

        endpoint = self._build_endpoint("traces")
        headers = (
            {"Authorization": f"Bearer {self._config.api_key}"} if self._config.api_key else {}
        )

        exporter = OTLPSpanExporter(endpoint=endpoint, headers=headers, timeout=30)
        self._otlp_processor = BatchSpanProcessor(
            exporter,
            max_queue_size=2048,
            schedule_delay_millis=5000,
            max_export_batch_size=512,
            export_timeout_millis=30000,
        )
        self.tracer_provider.add_span_processor(self._otlp_processor)

    def _build_endpoint(self, signal: str) -> str:
        """Build endpoint URL for a signal (traces, metrics, logs).

        If endpoint has a path (e.g., /otel/default), append /v1/{signal}.
        If no path, use SideSeat format: /otel/{project}/v1/{signal}.
        """
        parsed = urlparse(self._config.endpoint)
        if parsed.path and parsed.path != "/":
            # Endpoint has path - use as-is and append signal
            return f"{self._config.endpoint}/v1/{signal}"
        # No path - use SideSeat format
        return f"{self._config.endpoint}/otel/{self._config.project_id}/v1/{signal}"

    def _setup_metrics(self) -> None:
        """Set up OTLP metric export."""
        from opentelemetry import metrics
        from opentelemetry.exporter.otlp.proto.http.metric_exporter import (
            OTLPMetricExporter,
        )
        from opentelemetry.sdk.metrics import MeterProvider
        from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader

        from sideseat.telemetry.encoding import get_otel_resource

        endpoint = self._build_endpoint("metrics")
        headers = (
            {"Authorization": f"Bearer {self._config.api_key}"} if self._config.api_key else {}
        )

        exporter = OTLPMetricExporter(endpoint=endpoint, headers=headers)
        reader = PeriodicExportingMetricReader(exporter, export_interval_millis=60000)
        resource = get_otel_resource(self._config.service_name, self._config.service_version)
        self.meter_provider = MeterProvider(resource=resource, metric_readers=[reader])
        metrics.set_meter_provider(self.meter_provider)

    def _setup_logs(self) -> None:
        """Set up OTLP log export via Python logging bridge."""
        import logging as stdlib_logging

        # These imports may fail on older OTEL versions - handled by caller's try/except
        from opentelemetry._logs import set_logger_provider
        from opentelemetry.sdk._logs import LoggerProvider, LoggingHandler
        from opentelemetry.sdk._logs.export import BatchLogRecordProcessor

        try:
            from opentelemetry.exporter.otlp.proto.http._log_exporter import (
                OTLPLogExporter,
            )
        except ImportError:
            # Fall back to public API if available
            from opentelemetry.exporter.otlp.proto.http import (  # type: ignore[attr-defined,no-redef]
                LogExporter as OTLPLogExporter,
            )

        from sideseat.telemetry.encoding import get_otel_resource

        endpoint = self._build_endpoint("logs")
        headers = (
            {"Authorization": f"Bearer {self._config.api_key}"} if self._config.api_key else {}
        )

        resource = get_otel_resource(self._config.service_name, self._config.service_version)
        self.logger_provider = LoggerProvider(resource=resource)
        set_logger_provider(self.logger_provider)

        exporter = OTLPLogExporter(endpoint=endpoint, headers=headers)
        self.logger_provider.add_log_record_processor(BatchLogRecordProcessor(exporter))

        # Bridge Python logging to OTEL - store handler for cleanup
        self._logging_handler = LoggingHandler(
            level=stdlib_logging.NOTSET, logger_provider=self.logger_provider
        )
        stdlib_logging.getLogger().addHandler(self._logging_handler)

    def get_tracer(self, name: str = "sideseat", version: str | None = None) -> Tracer:
        """Get tracer for custom spans."""
        from sideseat._version import __version__

        tracer: Tracer = self.tracer_provider.get_tracer(name, version or __version__)
        return tracer

    @contextmanager
    def span(self, name: str, **kwargs: Any) -> Iterator[Span]:
        """Context manager for spans with auto error status."""
        with self.get_tracer().start_as_current_span(name, **kwargs) as s:
            try:
                yield s
            except Exception as e:
                s.set_status(Status(StatusCode.ERROR, str(e)))
                s.record_exception(e)
                raise

    def setup_console_exporter(self, **kwargs: Any) -> TelemetryClient:
        """Add console exporter for debugging."""
        if self._disabled:
            return self
        self.tracer_provider.add_span_processor(SimpleSpanProcessor(ConsoleSpanExporter(**kwargs)))
        return self

    def setup_file_exporter(self, path: str = "traces.jsonl", mode: str = "a") -> TelemetryClient:
        """Add JSONL file exporter."""
        if self._disabled:
            return self
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
