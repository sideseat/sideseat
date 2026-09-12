"""Telemetry setup for Anthropic samples."""

from sideseat import Frameworks, SideSeat


def setup_telemetry():
    """Initialize telemetry for Anthropic samples.

    SideSeat uses logfire to capture Anthropic API calls
    (messages, streaming) with full message events.
    """
    client = SideSeat(framework=Frameworks.Anthropic)
    client.telemetry.setup_console_exporter()
    return client
