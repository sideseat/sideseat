"""CrewAI's OpenTelemetry setup: the OpenInference CrewAI instrumentor on an OTLP-exporting provider."""

from openinference.instrumentation.crewai import CrewAIInstrumentor

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    CrewAIInstrumentor().instrument(tracer_provider=native.provider())
