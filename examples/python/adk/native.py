"""ADK's documented OpenTelemetry setup: traces and logs through the global providers.

`google.adk.telemetry.setup.maybe_set_otel_providers` gives an OTLP exporter to the tracer provider and to the
logger provider alike, and ADK writes each model call's messages as GenAI log events on that call's span - so a
setup exporting traces alone keeps every `generate_content` span and none of what it carried.
"""

from opentelemetry._logs import set_logger_provider

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    native.provider()
    set_logger_provider(native.logger_provider())
