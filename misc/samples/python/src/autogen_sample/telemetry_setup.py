"""Telemetry setup for AutoGen samples."""

from openinference.instrumentation.autogen_agentchat import AutogenAgentChatInstrumentor
from sideseat import Frameworks

from common.telemetry import setup_base_telemetry


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry with AutoGen instrumentation.

    Default: OpenTelemetry with console and OTLP exporters.
    Optional: SideSeat SDK with automatic OTLP setup + file exporter.
    """
    return setup_base_telemetry(
        instrumentor=lambda: AutogenAgentChatInstrumentor().instrument(),
        use_sideseat=use_sideseat,
        framework=Frameworks.AutoGen,
    )
