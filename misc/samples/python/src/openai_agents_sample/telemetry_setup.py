"""Telemetry setup for OpenAI Agents samples.

OpenAI Agents SDK uses logfire for instrumentation, which requires
special handling since it manages its own TracerProvider.
"""

import os

import logfire
from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.trace.export import BatchSpanProcessor, ConsoleSpanExporter


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry with logfire instrumentation.

    Uses logfire for OpenAI Agents SDK, then adds OTLP exporter.
    Note: This doesn't use common.telemetry because logfire manages its own provider.
    """
    service_name = os.getenv("OTEL_SERVICE_NAME", "openai-agents-sample")

    if use_sideseat:
        from sideseat import Frameworks, SideSeat

        # SideSeat handles logfire.configure() + instrument_openai_agents() internally
        client = SideSeat(framework=Frameworks.OpenAIAgents)
        client.telemetry.setup_file_exporter()
        client.telemetry.setup_console_exporter()
        return client
    else:
        # Configure logfire first - it sets up the TracerProvider
        logfire.configure(
            service_name=service_name,
            send_to_logfire=False,
            console=False,
        )

        # Instrument OpenAI Agents SDK
        logfire.instrument_openai_agents()

        # Add our exporters to logfire's provider
        provider = trace.get_tracer_provider()
        if hasattr(provider, "add_span_processor"):
            provider.add_span_processor(BatchSpanProcessor(ConsoleSpanExporter()))
            if os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT"):
                provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))

        return provider
