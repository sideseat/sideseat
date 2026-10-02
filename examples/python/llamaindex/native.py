"""LlamaIndex's OpenTelemetry setup: the OpenInference LlamaIndex instrumentor on an OTLP-exporting provider."""

from openinference.instrumentation.llama_index import LlamaIndexInstrumentor

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    LlamaIndexInstrumentor().instrument(tracer_provider=native.provider())
