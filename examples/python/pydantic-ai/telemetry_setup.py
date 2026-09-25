"""Telemetry setup for Pydantic AI samples."""

from common.telemetry import setup_logfire_telemetry
from sideseat import Frameworks, SideSeat


def setup_telemetry(use_sideseat: bool = False):
    """Initialize Pydantic AI telemetry in native or SideSeat mode."""
    if use_sideseat:
        client = SideSeat(framework=Frameworks.PydanticAI)
        client.telemetry.setup_console_exporter()
        return client
    return setup_logfire_telemetry("instrument_pydantic_ai", "pydantic-ai-sample")
