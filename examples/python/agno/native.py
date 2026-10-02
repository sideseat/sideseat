"""Agno's documented OpenTelemetry setup: the OpenInference Agno instrumentor on an OTLP provider."""

from openinference.instrumentation.agno import AgnoInstrumentor

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    AgnoInstrumentor().instrument(tracer_provider=native.provider())
