"""Telemetry setup for OpenAI samples."""

from sideseat import Frameworks, SideSeat


def setup_telemetry():
    """Initialize telemetry for OpenAI samples.

    SideSeat uses logfire to capture OpenAI API calls
    (Chat Completions and Responses API) with full message events.
    """
    client = SideSeat(framework=Frameworks.OpenAI)
    client.telemetry.setup_console_exporter()
    return client
