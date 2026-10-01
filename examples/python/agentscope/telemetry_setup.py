"""Telemetry setup for AgentScope captures."""

from typing import Any

from common.telemetry import NativeTraceClient, setup_base_telemetry
from sideseat import Frameworks


def setup_telemetry(use_sideseat: bool = False) -> Any:
    """Configure AgentScope's native middleware or SideSeat integration."""
    owner = setup_base_telemetry(
        use_sideseat=use_sideseat,
        framework=Frameworks.AgentScope,
    )
    if use_sideseat:
        return owner
    return NativeTraceClient(owner, "agentscope-sample")
