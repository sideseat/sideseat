"""LangChain's OpenTelemetry setup: the OpenInference LangChain instrumentor on an OTLP-exporting provider."""

from openinference.instrumentation.langchain import LangChainInstrumentor

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    LangChainInstrumentor().instrument(tracer_provider=native.provider())
