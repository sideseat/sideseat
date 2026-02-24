"""OTLP exporter, metrics, logs, and propagator setup."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any
from urllib.parse import urlparse

if TYPE_CHECKING:
    from sideseat.config import Config


def build_endpoint(config: Config, signal: str) -> str:
    """Build endpoint URL for a signal (traces, metrics, logs).

    If endpoint has a path (e.g., /otel/default), append /v1/{signal}.
    If no path, use SideSeat format: /otel/{project}/v1/{signal}.
    """
    parsed = urlparse(config.endpoint)
    if parsed.path and parsed.path != "/":
        return f"{config.endpoint}/v1/{signal}"
    return f"{config.endpoint}/otel/{config.project_id}/v1/{signal}"


def build_headers(config: Config) -> dict[str, str]:
    """Build auth headers from config."""
    if config.api_key:
        return {"Authorization": f"Bearer {config.api_key}"}
    return {}


def setup_otlp(config: Config, provider: Any) -> Any:
    """Set up OTLP trace export. Returns BatchSpanProcessor or None."""
    if not config.enable_traces:
        return None

    from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
    from opentelemetry.sdk.trace.export import BatchSpanProcessor

    endpoint = build_endpoint(config, "traces")
    headers = build_headers(config)

    exporter = OTLPSpanExporter(endpoint=endpoint, headers=headers, timeout=30)
    processor = BatchSpanProcessor(
        exporter,
        max_queue_size=2048,
        schedule_delay_millis=5000,
        max_export_batch_size=512,
        export_timeout_millis=30000,
    )
    provider.add_span_processor(processor)
    return processor


def setup_metrics(config: Config) -> Any:
    """Set up OTLP metric export. Returns MeterProvider."""
    from opentelemetry import metrics
    from opentelemetry.exporter.otlp.proto.http.metric_exporter import OTLPMetricExporter
    from opentelemetry.sdk.metrics import MeterProvider
    from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader

    from sideseat.telemetry.resource import get_otel_resource

    endpoint = build_endpoint(config, "metrics")
    headers = build_headers(config)

    exporter = OTLPMetricExporter(endpoint=endpoint, headers=headers)
    reader = PeriodicExportingMetricReader(exporter, export_interval_millis=60000)
    resource = get_otel_resource(config.service_name, config.service_version)
    meter_provider = MeterProvider(resource=resource, metric_readers=[reader])
    metrics.set_meter_provider(meter_provider)
    return meter_provider


def setup_logs(config: Config) -> tuple[Any, Any]:
    """Set up OTLP log export via Python logging bridge.

    Returns (LoggerProvider, LoggingHandler).
    """
    import logging as stdlib_logging

    from opentelemetry._logs import set_logger_provider
    from opentelemetry.sdk._logs import LoggerProvider, LoggingHandler
    from opentelemetry.sdk._logs.export import BatchLogRecordProcessor

    try:
        from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
    except ImportError:
        from opentelemetry.exporter.otlp.proto.http import (  # type: ignore[attr-defined,no-redef]
            LogExporter as OTLPLogExporter,
        )

    from sideseat.telemetry.resource import get_otel_resource

    endpoint = build_endpoint(config, "logs")
    headers = build_headers(config)

    resource = get_otel_resource(config.service_name, config.service_version)
    logger_provider = LoggerProvider(resource=resource)
    set_logger_provider(logger_provider)

    exporter = OTLPLogExporter(endpoint=endpoint, headers=headers)
    logger_provider.add_log_record_processor(BatchLogRecordProcessor(exporter))

    logging_handler = LoggingHandler(level=stdlib_logging.NOTSET, logger_provider=logger_provider)
    stdlib_logging.getLogger().addHandler(logging_handler)
    return logger_provider, logging_handler


def setup_propagators() -> None:
    """Set up W3C trace context and baggage propagators."""
    from opentelemetry import propagate
    from opentelemetry.baggage.propagation import W3CBaggagePropagator
    from opentelemetry.propagators.composite import CompositePropagator
    from opentelemetry.trace.propagation.tracecontext import TraceContextTextMapPropagator

    propagate.set_global_textmap(
        CompositePropagator(
            [
                W3CBaggagePropagator(),
                TraceContextTextMapPropagator(),
            ]
        )
    )
