"""Smolagents' documented OpenTelemetry setup: the OpenInference instrumentor on an OTLP provider."""

from openinference.instrumentation.smolagents import SmolagentsInstrumentor

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    SmolagentsInstrumentor().instrument(tracer_provider=native.provider())
