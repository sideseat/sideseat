"""Semantic Kernel's documented OpenTelemetry setup: its GenAI diagnostics on, traces and logs exported.

Semantic Kernel reads the diagnostics switches when its modules are first imported, so they are set
before anything imports it. It writes prompts and completions as Python log records, so a logging
handler sends its records to an OTLP log exporter beside the traces.
"""

import logging
import os

from opentelemetry._logs import set_logger_provider
from opentelemetry.sdk._logs import LoggingHandler

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    os.environ["SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS"] = "true"
    os.environ[
        "SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS_SENSITIVE"
    ] = "true"
    native.provider()
    logs = native.logger_provider()
    set_logger_provider(logs)
    kernel_logs = logging.getLogger("semantic_kernel")
    kernel_logs.addHandler(LoggingHandler(logger_provider=logs))
    kernel_logs.setLevel(logging.INFO)
