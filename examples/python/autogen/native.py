"""AutoGen's OpenTelemetry setup: the OpenInference AgentChat instrumentor on an OTLP-exporting provider.

The provider is also the global one, which is where AgentChat's own GenAI spans go.
"""

from openinference.instrumentation.autogen_agentchat import AutogenAgentChatInstrumentor

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    AutogenAgentChatInstrumentor().instrument(tracer_provider=native.provider())
