"""Telemetry setup for Google ADK samples."""

from sideseat import Frameworks

from common.telemetry import setup_base_telemetry


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry for Google ADK.

    Google ADK has built-in OpenTelemetry support, so no instrumentor needed.
    Default: OpenTelemetry with console and OTLP exporters.
    Optional: SideSeat SDK with automatic OTLP setup + file exporter.
    """
    return setup_base_telemetry(
        instrumentor=None,  # ADK has built-in telemetry
        use_sideseat=use_sideseat,
        framework=Frameworks.GoogleADK,
    )
