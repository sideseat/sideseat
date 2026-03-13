"""Telemetry setup for OpenAI Agents samples.

OpenAI Agents SDK uses logfire for instrumentation, which manages its own
TracerProvider. logfire>=4.29.0 provides instrument_openai_agents() using
the official agents SDK hook (set_trace_provider).
"""

import os

import logfire
from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.trace.export import BatchSpanProcessor, ConsoleSpanExporter
from sideseat import Frameworks, SideSeat


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry with logfire instrumentation.

    Uses logfire for OpenAI Agents SDK, then adds OTLP exporter.
    Note: This doesn't use common.telemetry because logfire manages its own provider.
    """
    service_name = os.getenv("OTEL_SERVICE_NAME", "openai-agents-sample")

    if use_sideseat:
        # SideSeat handles logfire.configure() + instrument_openai_agents() internally
        client = SideSeat(framework=Frameworks.OpenAIAgents)
        client.telemetry.setup_console_exporter()
        return client
    else:
        # Configure logfire first - it sets up the TracerProvider
        logfire.configure(
            service_name=service_name,
            send_to_logfire=False,
            console=False,
        )

        # Instrument OpenAI Agents SDK via official SDK hook
        logfire.instrument_openai_agents()

        # Add our exporters to logfire's provider
        provider = trace.get_tracer_provider()
        if hasattr(provider, "add_span_processor"):
            provider.add_span_processor(BatchSpanProcessor(ConsoleSpanExporter()))
            sideseat_base = os.getenv(
                "SIDESEAT_ENDPOINT", "http://127.0.0.1:5388"
            ).rstrip("/")
            project_id = os.getenv("SIDESEAT_PROJECT_ID", "default")
            endpoint = f"{sideseat_base}/otel/{project_id}/v1/traces"
            provider.add_span_processor(
                BatchSpanProcessor(OTLPSpanExporter(endpoint=endpoint))
            )

        return provider
