"""Telemetry setup for LangChain captures."""

from typing import Any

from openinference.instrumentation.langchain import LangChainInstrumentor
from sideseat import Frameworks

from common.telemetry import NativeTraceClient, setup_base_telemetry


def setup_telemetry(use_sideseat: bool = False) -> Any:
    """Configure native OpenInference or SideSeat-owned instrumentation."""

    def instrumentor(provider: Any = None) -> None:
        LangChainInstrumentor().instrument(
            tracer_provider=provider,
            skip_dep_check=True,
        )

    owner = setup_base_telemetry(
        instrumentor=instrumentor,
        use_sideseat=use_sideseat,
        framework=Frameworks.LangChain,
    )
    if use_sideseat:
        return owner
    return NativeTraceClient(owner, "langchain-sample")
