"""Telemetry setup for Bedrock samples."""

from opentelemetry.instrumentation.botocore import BotocoreInstrumentor
from sideseat import Frameworks, SideSeat

from common.telemetry import NativeTraceClient, setup_base_telemetry


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry for Bedrock samples.

    Native mode uses the upstream botocore instrumentor. SideSeat mode uses the
    SDK's Bedrock-specific instrumentation and OTLP pipeline.
    """
    if use_sideseat:
        client = SideSeat(framework=Frameworks.Bedrock)
        client.telemetry.setup_console_exporter()
        return client

    provider = setup_base_telemetry(
        instrumentor=lambda tracer_provider: BotocoreInstrumentor().instrument(
            tracer_provider=tracer_provider
        ),
    )
    return NativeTraceClient(provider, "bedrock-sample")
