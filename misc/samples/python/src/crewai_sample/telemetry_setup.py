"""Telemetry setup for CrewAI samples."""

from openinference.instrumentation.crewai import CrewAIInstrumentor
from sideseat import Frameworks

from common.telemetry import setup_base_telemetry


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry with CrewAI instrumentation.

    Default: OpenTelemetry with console and OTLP exporters.
    Optional: SideSeat SDK with automatic OTLP setup + file exporter.
    """
    return setup_base_telemetry(
        instrumentor=lambda: CrewAIInstrumentor().instrument(),
        use_sideseat=use_sideseat,
        framework=Frameworks.CrewAI,
    )
