"""Semantic Kernel's documented OpenTelemetry setup: its GenAI diagnostics switched on, a global provider.

Semantic Kernel reads the diagnostics switches when its modules are first imported, so they are set
before anything imports it.
"""

import os

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    os.environ["SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS"] = "true"
    os.environ[
        "SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS_SENSITIVE"
    ] = "true"
    native.provider()
