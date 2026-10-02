"""AG2's OpenTelemetry setup: an OTLP-exporting global provider for its TelemetryMiddleware.

AG2 traces through middleware attached to each agent; `agent.py` attaches it in native mode.
"""

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    native.provider()
