"""Agent Framework's documented setup on an existing OpenTelemetry provider: enable_sensitive_telemetry.

Agent Framework instruments itself; with the provider configured by the application, the
documentation asks only for sensitive data capture to be switched on.
"""

from agent_framework.observability import enable_sensitive_telemetry

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    native.provider()
    enable_sensitive_telemetry()
