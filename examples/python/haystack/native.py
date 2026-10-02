"""Haystack's OpenTelemetry setup: its OpenTelemetry tracer on an OTLP-exporting provider."""

from haystack import tracing
from haystack_integrations.tracing.opentelemetry import OpenTelemetryTracer

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    # Haystack records prompts and replies only with content tracing on; the switch is read when
    # the tracing module is imported, so it is set on the tracer rather than in the environment.
    tracing.tracer.is_content_tracing_enabled = True
    tracing.enable_tracing(
        OpenTelemetryTracer(native.provider().get_tracer("haystack"))
    )
