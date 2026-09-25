"""Telemetry setup for Google GenAI samples."""

import os

from common.telemetry import setup_logfire_telemetry
from sideseat import Frameworks, SideSeat


def setup_telemetry(use_sideseat: bool = False):
    """Initialize Google GenAI telemetry in native or SideSeat mode."""
    # The upstream instrumentor defaults to no message content. Use the same
    # public setting that SideSeat(capture_content=True) applies.
    os.environ.setdefault("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "true")

    if use_sideseat:
        client = SideSeat(framework=Frameworks.GoogleGenAI)
        client.telemetry.setup_console_exporter()
        return client
    return setup_logfire_telemetry("instrument_google_genai", "google-genai-sample")
