"""Telemetry setup for Google Vertex AI captures."""

import os
from typing import Any

from common.telemetry import setup_logfire_telemetry
from sideseat import Frameworks, SideSeat

SERVICE_NAME = "vertex-ai-sample"


def setup_telemetry(use_sideseat: bool = False) -> Any:
    """Configure Google Gen AI instrumentation directly or through SideSeat."""
    os.environ.setdefault("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "true")

    if use_sideseat:
        owner = SideSeat(framework=Frameworks.VertexAI)
        owner.telemetry.setup_console_exporter()
        return owner
    return setup_logfire_telemetry("instrument_google_genai", SERVICE_NAME)
