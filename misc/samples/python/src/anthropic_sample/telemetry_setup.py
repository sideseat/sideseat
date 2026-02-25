"""Telemetry setup for Anthropic samples."""

from sideseat import Frameworks, SideSeat


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry for Anthropic samples.

    SideSeat uses logfire to capture Anthropic API calls
    (messages, streaming) with full message events.
    """
    if use_sideseat:
        client = SideSeat(framework=Frameworks.Anthropic)
        # client.telemetry.setup_file_exporter()
        client.telemetry.setup_console_exporter()
        return client

    client = SideSeat(framework=Frameworks.Anthropic)
    client.telemetry.setup_console_exporter()
    return client
