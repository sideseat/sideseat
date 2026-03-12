"""Telemetry setup for Microsoft Agent Framework samples.

Agent Framework has built-in OpenTelemetry support that automatically
creates spans using GenAI semantic conventions when a TracerProvider is
configured. No separate instrumentor is needed.
"""

from agent_framework.observability import OBSERVABILITY_SETTINGS
from common.telemetry import setup_base_telemetry
from sideseat import Frameworks


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry for Agent Framework samples.

    Default: OpenTelemetry with console and OTLP exporters.
    Optional: SideSeat SDK with automatic OTLP setup + console exporter.

    Agent Framework uses its own OBSERVABILITY_SETTINGS gate. We enable it
    here so spans flow through to the global OTel provider we configure below.
    """
    # Enable agent_framework built-in OTel instrumentation
    OBSERVABILITY_SETTINGS.enable_otel = True
    OBSERVABILITY_SETTINGS.enable_sensitive_data = True

    return setup_base_telemetry(
        instrumentor=None,  # Agent Framework self-instruments via global OTel provider
        use_sideseat=use_sideseat,
        framework=Frameworks.AgentFramework,
    )
