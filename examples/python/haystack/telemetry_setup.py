"""Telemetry setup for Haystack captures."""

from typing import Any

from common.telemetry import NativeTraceClient, setup_base_telemetry
from haystack import tracing
from haystack_integrations.tracing.opentelemetry import OpenTelemetryTracer
from sideseat import Frameworks


def setup_telemetry(use_sideseat: bool = False) -> Any:
    """Configure native Haystack tracing or SideSeat-owned instrumentation."""

    def instrumentor(provider: Any = None) -> None:
        tracing.tracer.is_content_tracing_enabled = True
        tracing.enable_tracing(OpenTelemetryTracer(provider.get_tracer("haystack")))

    owner = setup_base_telemetry(
        instrumentor=instrumentor,
        use_sideseat=use_sideseat,
        framework=Frameworks.Haystack,
    )
    if use_sideseat:
        return owner
    return NativeTraceClient(owner, "haystack-sample")
