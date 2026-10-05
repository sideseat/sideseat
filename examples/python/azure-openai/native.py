"""OpenInference's documented OpenAI instrumentor on a plain OpenTelemetry provider.

The instrumentor covers ``AzureOpenAI`` as well as ``OpenAI``, and reports Azure as the provider
from the client's endpoint. SideSeat's ``azure-openai`` integration installs the same instrumentor.
"""

from openinference.instrumentation.openai import OpenAIInstrumentor

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    OpenAIInstrumentor().instrument(tracer_provider=native.provider())
