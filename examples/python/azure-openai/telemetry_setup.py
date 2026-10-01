"""Telemetry setup for Azure OpenAI captures."""

from typing import Any

from openinference.instrumentation.openai import OpenAIInstrumentor
from sideseat import Frameworks

from common.telemetry import NativeTraceClient, setup_base_telemetry

SERVICE_NAME = "azure-openai-sample"


def _instrument_native(provider: Any) -> None:
    """Bind OpenInference's OpenAI instrumentor to the native provider."""
    OpenAIInstrumentor().instrument(tracer_provider=provider, skip_dep_check=True)


def setup_telemetry(use_sideseat: bool = False) -> Any:
    """Configure OpenInference directly or through SideSeat."""
    owner = setup_base_telemetry(
        instrumentor=_instrument_native,
        use_sideseat=use_sideseat,
        framework=Frameworks.AzureOpenAI,
    )
    if use_sideseat:
        return owner
    return NativeTraceClient(owner, SERVICE_NAME)
