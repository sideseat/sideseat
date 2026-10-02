"""LangChain and LangGraph, through the OpenInference LangChain instrumentor."""

from __future__ import annotations

from typing import ClassVar

from sideseat.integrations.openinference import OpenInferenceIntegration


class LangChain(OpenInferenceIntegration):
    name = "langchain"
    packages: ClassVar[tuple[str, ...]] = ("langchain-core", "langchain")
    extra = "langchain"
    instrumentor_module = "langchain"
    instrumentor_class = "LangChainInstrumentor"


class LangGraph(LangChain):
    name = "langgraph"
    packages = ("langgraph",)
    extra = "langgraph"
