"""Telemetry setup for Semantic Kernel captures."""

import os
from typing import Any

from common.telemetry import NativeTraceClient, setup_base_telemetry
from sideseat import Frameworks


def setup_telemetry(use_sideseat: bool = False) -> Any:
    """Configure Semantic Kernel's native diagnostics or SideSeat integration."""
    os.environ.setdefault(
        "SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS",
        "true",
    )
    os.environ.setdefault(
        "SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS_SENSITIVE",
        "true",
    )

    owner = setup_base_telemetry(
        use_sideseat=use_sideseat,
        framework=Frameworks.SemanticKernel,
    )
    if use_sideseat:
        return owner
    return NativeTraceClient(owner, "semantic-kernel-sample")
