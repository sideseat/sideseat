"""AgentScope's documented OpenTelemetry setup: a global OTLP provider, and TracingMiddleware on each agent.

The middleware half lives in :func:`agents.agent`, which adds it to every agent in native mode.
"""

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    native.provider()
