"""The function-calling agent the scenarios run."""

from collections.abc import Sequence
from typing import Any

from llama_index.core.agent.workflow import FunctionAgent
from llama_index.core.llms import LLM
from llama_index.core.tools import BaseTool

from harness import content


def build_agent(
    llm: LLM,
    *,
    tools: Sequence[BaseTool] = (),
    system: str | None = content.SYSTEM,
    **options: Any,
) -> FunctionAgent:
    return FunctionAgent(
        name="assistant",
        description="A travel assistant.",
        llm=llm,
        tools=list(tools),
        system_prompt=system,
        **options,
    )
