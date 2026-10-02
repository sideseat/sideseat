"""The agent the scenarios run, and how its answer is read."""

from collections.abc import Sequence
from typing import Any

from haystack.components.agents import Agent
from haystack.components.generators.chat.types import ChatGenerator
from haystack.dataclasses import ChatMessage
from haystack.tools import Tool, Toolset

from harness import content


def build_agent(
    llm: ChatGenerator,
    *,
    tools: Sequence[Tool | Toolset] = (),
    system: str | None = content.SYSTEM,
) -> Agent:
    return Agent(chat_generator=llm, tools=list(tools), system_prompt=system)


def ask(agent: Agent, *messages: ChatMessage) -> dict[str, Any]:
    """Runs the agent on ``messages``; the result holds the whole conversation."""
    return agent.run(messages=list(messages))


def answer(result: dict[str, Any]) -> str:
    return str(result["last_message"].text)
