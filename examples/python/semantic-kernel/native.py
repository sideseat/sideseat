"""Semantic Kernel's documented OpenTelemetry setup: its GenAI diagnostics on, traces and logs exported.

Semantic Kernel reads the diagnostics switches when its modules are first imported, so they are set
before anything imports it. It writes prompts and completions as Python log records, so a logging
handler sends its records to an OTLP log exporter beside the traces.
"""

import logging
import os

from opentelemetry._logs import set_logger_provider
from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
from opentelemetry.sdk._logs import LoggerProvider, LoggingHandler
from opentelemetry.sdk._logs.export import BatchLogRecordProcessor

from harness.telemetry import NativeTelemetry, auth_headers, traces_endpoint


def configure(native: NativeTelemetry) -> None:
    os.environ["SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS"] = "true"
    os.environ[
        "SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS_SENSITIVE"
    ] = "true"
    native.provider()
    logs = LoggerProvider()
    logs.add_log_record_processor(
        BatchLogRecordProcessor(
            OTLPLogExporter(
                endpoint=traces_endpoint().removesuffix("/v1/traces") + "/v1/logs",
                headers=auth_headers(),
            )
        )
    )
    set_logger_provider(logs)
    kernel_logs = logging.getLogger("semantic_kernel")
    kernel_logs.addHandler(LoggingHandler(logger_provider=logs))
    kernel_logs.setLevel(logging.INFO)
