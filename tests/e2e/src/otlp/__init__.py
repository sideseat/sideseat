"""OTLP (OpenTelemetry Protocol) test namespace."""

from .traces import (
    create_otlp_trace,
    generate_span_id,
    generate_trace_id,
    send_otlp_traces_http,
)

__all__ = [
    "create_otlp_trace",
    "generate_span_id",
    "generate_trace_id",
    "send_otlp_traces_http",
]
