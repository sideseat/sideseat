"""OpenTelemetry's botocore instrumentation, which is what a boto3 application uses without SideSeat.

The instrumentation records Bedrock calls as spans and their messages as GenAI log events, so the
setup exports logs beside traces.
"""

from opentelemetry._logs import set_logger_provider
from opentelemetry.instrumentation.botocore import BotocoreInstrumentor

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    logs = native.logger_provider()
    set_logger_provider(logs)
    BotocoreInstrumentor().instrument(
        tracer_provider=native.provider(), logger_provider=logs
    )
