"""Telemetry setup for AG2 captures."""

from typing import Any

from common.telemetry import NativeTraceClient, setup_base_telemetry
from sideseat import Frameworks


def setup_telemetry(use_sideseat: bool = False) -> Any:
    """Configure AG2's native middleware or SideSeat integration."""
    owner = setup_base_telemetry(
        use_sideseat=use_sideseat,
        framework=Frameworks.AG2,
    )
    if use_sideseat:
        return owner
    return NativeTraceClient(owner, "ag2-sample")
