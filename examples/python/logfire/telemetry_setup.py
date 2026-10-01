"""Telemetry setup for generic Logfire captures."""

from typing import Any

from common.telemetry import setup_logfire_telemetry
from sideseat import Frameworks, SideSeat

SERVICE_NAME = "logfire-sample"


def setup_telemetry(use_sideseat: bool = False) -> Any:
    """Configure Logfire directly or through SideSeat's generic integration."""
    if use_sideseat:
        client = SideSeat(
            framework=Frameworks.Logfire,
            service_name=SERVICE_NAME,
        )
        client.telemetry.setup_console_exporter()
        return client
    return setup_logfire_telemetry(None, SERVICE_NAME)
