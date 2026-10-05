"""ADK's documented OpenTelemetry setup: ADK traces through the global tracer provider."""

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    native.provider()
