"""Telemetry setup for generic OpenInference captures."""

from importlib.metadata import version
from typing import Any

from common.telemetry import NativeTraceClient, setup_base_telemetry
from openinference.instrumentation import OITracer, TraceConfig
from sideseat import SideSeat

SERVICE_NAME = "openinference-sample"
SCOPE_NAME = "openinference.instrumentation.sideseat_sample"


def setup_telemetry(use_sideseat: bool = False) -> tuple[Any, OITracer]:
    """Configure the official OpenInference tracer over native or SideSeat OTel."""
    if use_sideseat:
        owner = SideSeat(
            auto_instrument=False,
            service_name=SERVICE_NAME,
        )
        owner.telemetry.setup_console_exporter()
    else:
        provider = setup_base_telemetry()
        owner = NativeTraceClient(provider, SERVICE_NAME)

    tracer = OITracer(
        owner.tracer_provider.get_tracer(
            SCOPE_NAME,
            version("openinference-instrumentation"),
        ),
        TraceConfig(),
    )
    return owner, tracer
