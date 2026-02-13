"""Telemetry setup for LangGraph samples."""

import os

from openinference.instrumentation.langchain import LangChainInstrumentor
from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.instrumentation.botocore import BotocoreInstrumentor
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor, ConsoleSpanExporter


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry with standard configuration.

    Sets up OpenTelemetry with console and OTLP exporters.
    Instruments LangChain/LangGraph and boto3/botocore for tracing.
    """
    # Configure OpenTelemetry
    provider = TracerProvider()
    provider.add_span_processor(BatchSpanProcessor(ConsoleSpanExporter()))

    if os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT"):
        provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))

    trace.set_tracer_provider(provider)

    # Instrument LangChain (covers LangGraph)
    LangChainInstrumentor().instrument()

    # Instrument boto3/botocore for AWS call tracing
    BotocoreInstrumentor().instrument()

    if use_sideseat:
        # SideSeat automatically sets up OTLP traces, metrics, and logs
        from sideseat import Frameworks, SideSeat

        client = SideSeat(framework=Frameworks.LangChain)
        client.telemetry.setup_file_exporter()
        client.telemetry.setup_console_exporter()
        return client

    return provider
