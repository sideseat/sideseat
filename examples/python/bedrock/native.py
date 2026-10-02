"""OpenTelemetry's botocore instrumentation, which is what a boto3 application uses without SideSeat.

The instrumentation records Bedrock calls as spans and their messages as GenAI log events, so the
setup exports logs beside traces.
"""

from opentelemetry._logs import set_logger_provider
from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
from opentelemetry.instrumentation.botocore import BotocoreInstrumentor
from opentelemetry.sdk._logs import LoggerProvider
from opentelemetry.sdk._logs.export import BatchLogRecordProcessor

from harness.telemetry import NativeTelemetry, auth_headers, traces_endpoint


def configure(native: NativeTelemetry) -> None:
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
    BotocoreInstrumentor().instrument(
        tracer_provider=native.provider(), logger_provider=logs
    )
