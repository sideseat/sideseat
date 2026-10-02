"""Frameworks instrumented by an OpenInference instrumentor."""

from __future__ import annotations

import importlib
from typing import ClassVar

from sideseat.integrations._base import Integration, SetupContext


class OpenInferenceIntegration(Integration):
    """Installs ``openinference.instrumentation.<module>.<instrumentor>`` on SideSeat's provider."""

    instrumentor_module: ClassVar[str]
    instrumentor_class: ClassVar[str]

    def __init__(self) -> None:
        self._instrumentor: object | None = None

    def instrument(self, ctx: SetupContext) -> None:
        module = importlib.import_module(
            f"openinference.instrumentation.{self.instrumentor_module}"
        )
        instrumentor = getattr(module, self.instrumentor_class)()
        # skip_dep_check: the instrumentors refuse to run on any framework version newer than the
        # one they were released against, even when every patched symbol still exists. The
        # conformance fixtures are what prove a given version works.
        instrumentor.instrument(tracer_provider=ctx.tracer_provider, skip_dep_check=True)
        self._instrumentor = instrumentor

    def shutdown(self) -> None:
        if self._instrumentor is not None:
            self._instrumentor.uninstrument()  # type: ignore[attr-defined]
            self._instrumentor = None


class CrewAI(OpenInferenceIntegration):
    name = "crewai"
    packages = ("crewai",)
    extra = "crewai"
    instrumentor_module = "crewai"
    instrumentor_class = "CrewAIInstrumentor"


class AutoGen(OpenInferenceIntegration):
    name = "autogen"
    packages = ("autogen-agentchat",)
    extra = "autogen"
    instrumentor_module = "autogen_agentchat"
    instrumentor_class = "AutogenAgentChatInstrumentor"


class Agno(OpenInferenceIntegration):
    name = "agno"
    packages = ("agno",)
    extra = "agno"
    instrumentor_module = "agno"
    instrumentor_class = "AgnoInstrumentor"


class Smolagents(OpenInferenceIntegration):
    name = "smolagents"
    packages = ("smolagents",)
    extra = "smolagents"
    instrumentor_module = "smolagents"
    instrumentor_class = "SmolagentsInstrumentor"


class LlamaIndex(OpenInferenceIntegration):
    name = "llama-index"
    packages = ("llama-index-core",)
    extra = "llama-index"
    instrumentor_module = "llama_index"
    instrumentor_class = "LlamaIndexInstrumentor"


class AzureOpenAI(OpenInferenceIntegration):
    name = "azure-openai"
    packages = ("openai",)
    detectable = False
    extra = "azure-openai"
    instrumentor_module = "openai"
    instrumentor_class = "OpenAIInstrumentor"
