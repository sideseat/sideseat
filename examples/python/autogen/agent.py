"""The assistant agent the scenarios run."""

from collections.abc import Sequence
from typing import Any

from autogen_agentchat.agents import AssistantAgent
from autogen_core.models import ChatCompletionClient
from autogen_core.tools import BaseTool

from harness import content


def assistant(
    llm: ChatCompletionClient,
    *,
    tools: Sequence[BaseTool[Any, Any]] = (),
    system: str | None = content.SYSTEM,
    **options: Any,
) -> AssistantAgent:
    # Reflection sends tool results back to the model, so the answer is the model's, not the
    # tool's raw output.
    return AssistantAgent(
        "assistant",
        model_client=llm,
        system_message=system,
        tools=list(tools),
        reflect_on_tool_use=bool(tools),
        max_tool_iterations=5,
        **options,
    )


def answer(result: Any) -> str:
    """The text of the last message of a task result."""
    return str(result.messages[-1].to_text())
