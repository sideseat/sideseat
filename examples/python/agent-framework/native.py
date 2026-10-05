"""Agent Framework's documented setup on an existing OpenTelemetry provider: enable_sensitive_telemetry.

Agent Framework instruments itself; with the provider configured by the application, the
documentation asks only for sensitive data capture to be switched on. Releases before 1.8 document the
same switch as ``enable_instrumentation(enable_sensitive_data=True)``; the version matrix replays them.
"""

from agent_framework import observability

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    native.provider()
    if hasattr(observability, "enable_sensitive_telemetry"):
        observability.enable_sensitive_telemetry()
    else:
        observability.enable_instrumentation(enable_sensitive_data=True)
