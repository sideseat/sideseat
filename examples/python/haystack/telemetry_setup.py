"""Telemetry setup for Haystack captures."""

from typing import Any

from common.telemetry import NativeTraceClient, setup_base_telemetry
from openinference.instrumentation.haystack import HaystackInstrumentor
from sideseat import Frameworks


def setup_telemetry(use_sideseat: bool = False) -> Any:
    """Configure native OpenInference or SideSeat-owned instrumentation."""

    def instrumentor(provider: Any = None) -> None:
        HaystackInstrumentor().instrument(
            tracer_provider=provider,
            skip_dep_check=True,
        )

    owner = setup_base_telemetry(
        instrumentor=instrumentor,
        use_sideseat=use_sideseat,
        framework=Frameworks.Haystack,
    )
    if use_sideseat:
        return owner
    return NativeTraceClient(owner, "haystack-sample")
