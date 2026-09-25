"""Telemetry setup for OpenAI samples."""

from sideseat import Frameworks, SideSeat

from common.telemetry import setup_logfire_telemetry


def setup_telemetry(use_sideseat: bool = False):
    """Initialize telemetry for OpenAI samples.

    Native mode configures Logfire and a raw OTLP exporter. SideSeat mode delegates
    provider instrumentation and export pipeline ownership to the SDK.
    """
    if use_sideseat:
        client = SideSeat(framework=Frameworks.OpenAI)
        client.telemetry.setup_console_exporter()
        return client
    return setup_logfire_telemetry("instrument_openai", "openai-sample")
