"""Telemetry setup for Google ADK samples."""

from sideseat import Frameworks
from sideseat.instrumentation import patch_adk_tracing

from common.telemetry import setup_base_telemetry


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry for Google ADK.

    Google ADK has built-in OpenTelemetry support, so no instrumentor needed.
    Default: OpenTelemetry with console and OTLP exporters.
    Optional: SideSeat SDK with automatic OTLP setup + file exporter.
    """
    # ADK intentionally removes inline image/document bytes from its native trace payload. Apply
    # SideSeat's narrow compatibility patch in both conformance modes so native-vs-SDK comparison
    # isolates telemetry ownership rather than comparing complete input against upstream data loss.
    patch_adk_tracing()

    return setup_base_telemetry(
        instrumentor=None,  # ADK has built-in telemetry
        use_sideseat=use_sideseat,
        framework=Frameworks.GoogleADK,
    )
