"""Framework and provider integrations.

Pass names or instances to :func:`sideseat.init`::

    sideseat.init(integrations=["strands", "bedrock"])

    from sideseat.integrations import OpenAIAgents
    sideseat.init(integrations=[OpenAIAgents()])

Without ``integrations``, installed frameworks are detected. Provider client libraries are never
detected, because frameworks install them transitively; name them explicitly.
"""

from __future__ import annotations

import importlib
import logging
from collections.abc import Sequence
from typing import Any

from sideseat.errors import IntegrationError
from sideseat.integrations._base import Integration, SetupContext

logger = logging.getLogger("sideseat")

# name -> (module, class). Imported lazily: loading an integration module must not import its
# framework, and listing what exists must not import any of them.
_REGISTRY: dict[str, tuple[str, str]] = {
    # Agent frameworks, in auto-detection priority: a framework that depends on another is listed
    # before it, so LangGraph wins over LangChain and an application is labelled by what it uses.
    "strands": ("strands", "Strands"),
    "langgraph": ("langchain", "LangGraph"),
    "crewai": ("openinference", "CrewAI"),
    "autogen": ("openinference", "AutoGen"),
    "ag2": ("ag2", "AG2"),
    "openai-agents": ("logfire", "OpenAIAgents"),
    "google-adk": ("google_adk", "GoogleADK"),
    "pydantic-ai": ("logfire", "PydanticAI"),
    "agent-framework": ("agent_framework", "AgentFramework"),
    "semantic-kernel": ("semantic_kernel", "SemanticKernel"),
    "claude-agent-sdk": ("claude_agent_sdk", "ClaudeAgentSDK"),
    "agno": ("openinference", "Agno"),
    "smolagents": ("openinference", "Smolagents"),
    "llama-index": ("openinference", "LlamaIndex"),
    "agentscope": ("agentscope", "AgentScope"),
    "haystack": ("haystack", "Haystack"),
    "browser-use": ("browser_use", "BrowserUse"),
    "langflow": ("passthrough", "Langflow"),
    "langchain": ("langchain", "LangChain"),
    "traceloop": ("traceloop", "TraceLoop"),
    "logfire": ("logfire", "Logfire"),
    "openinference": ("passthrough", "OpenInference"),
    # Providers: explicit only.
    "bedrock": ("bedrock", "Bedrock"),
    "openai": ("logfire", "OpenAI"),
    "azure-openai": ("openinference", "AzureOpenAI"),
    "anthropic": ("logfire", "Anthropic"),
    "google-genai": ("logfire", "GoogleGenAI"),
    "vertex-ai": ("logfire", "VertexAI"),
}


def names() -> tuple[str, ...]:
    """Every integration name, in auto-detection priority order."""
    return tuple(_REGISTRY)


def load(name: str) -> type[Integration]:
    """The integration class registered under ``name``."""
    try:
        module_name, class_name = _REGISTRY[name]
    except KeyError:
        known = ", ".join(sorted(_REGISTRY))
        raise IntegrationError(
            f"unknown integration {name!r}; known integrations: {known}"
        ) from None
    module = importlib.import_module(f"sideseat.integrations.{module_name}")
    cls: type[Integration] = getattr(module, class_name)
    return cls


def resolve(spec: Sequence[Any] | None) -> tuple[list[Integration], bool]:
    """Integration instances for ``spec``, and whether they were requested explicitly.

    ``None`` detects installed frameworks. Duplicate names keep their first position.
    """
    if spec is None:
        return detect(), False
    result: list[Integration] = []
    seen: set[str] = set()
    for item in spec:
        integration = load(item)() if isinstance(item, str) else item
        if not isinstance(integration, Integration):
            raise IntegrationError(f"{item!r} is not an integration name or Integration instance")
        if integration.name not in seen:
            seen.add(integration.name)
            result.append(integration)
    return result, True


def detect() -> list[Integration]:
    """The installed framework that best describes the application, if any.

    Only one is chosen: several frameworks bundle others as dependencies, so presence of a second
    package is not evidence that the application uses it.
    """
    for name in _REGISTRY:
        cls = load(name)
        if cls.detectable and cls.installed_package() is not None:
            logger.debug("Detected integration %s", name)
            return [cls()]
    return []


def __getattr__(attr: str) -> Any:
    # `from sideseat.integrations import Strands` without importing every integration module.
    for module_name, class_name in _REGISTRY.values():
        if class_name == attr:
            module = importlib.import_module(f"sideseat.integrations.{module_name}")
            return getattr(module, class_name)
    raise AttributeError(attr)


__all__ = ["Integration", "SetupContext", "detect", "load", "names", "resolve"]
