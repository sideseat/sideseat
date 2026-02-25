"""Telemetry setup for OpenAI samples."""

from sideseat import Frameworks, SideSeat


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry for OpenAI samples.

    SideSeat uses openinference-instrumentation-openai to capture OpenAI API calls
    (Chat Completions and Responses API) with full message events.
    """
    if use_sideseat:
        client = SideSeat(framework=Frameworks.OpenAI)
        # client.telemetry.setup_file_exporter()
        client.telemetry.setup_console_exporter()
        return client

    client = SideSeat(framework=Frameworks.OpenAI)
    client.telemetry.setup_console_exporter()
    return client
