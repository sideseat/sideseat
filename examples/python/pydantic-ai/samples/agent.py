"""Deterministic Pydantic AI agent, history, and tool-use scenarios."""

from collections.abc import Sequence
from typing import Any

from pydantic_ai import Agent
from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    TextPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)
from pydantic_ai.models.function import AgentInfo, FunctionModel


def _latest_user_text(messages: Sequence[ModelMessage]) -> str:
    """Return the latest string user prompt in a model request."""
    for message in reversed(messages):
        if not isinstance(message, ModelRequest):
            continue
        for part in reversed(message.parts):
            if isinstance(part, UserPromptPart) and isinstance(part.content, str):
                return part.content
    return ""


def _has_tool_result(messages: Sequence[ModelMessage]) -> bool:
    """Whether the latest model request includes a tool result."""
    latest = messages[-1]
    return isinstance(latest, ModelRequest) and any(
        isinstance(part, ToolReturnPart) for part in latest.parts
    )


def _model(messages: list[ModelMessage], info: AgentInfo) -> ModelResponse:
    """Produce exact deterministic model responses for every scenario."""
    if _has_tool_result(messages):
        return ModelResponse(parts=[TextPart("It is sunny and 22°C in Paris.")])

    prompt = _latest_user_text(messages)
    if "weather" in prompt.lower() and info.function_tools:
        return ModelResponse(
            parts=[
                ToolCallPart(
                    "get_weather",
                    {"location": "Paris"},
                    tool_call_id="weather-call-1",
                )
            ]
        )
    if "speed of light" in prompt.lower():
        return ModelResponse(
            parts=[TextPart("The speed of light is 299,792,458 metres per second.")]
        )
    if "boiling point" in prompt.lower():
        return ModelResponse(parts=[TextPart("Water boils at 100°C at sea level.")])
    if "same scale" in prompt.lower():
        return ModelResponse(parts=[TextPart("That is 212°F on the Fahrenheit scale.")])
    return ModelResponse(parts=[TextPart("I do not have a deterministic answer.")])


def _agent(*, name: str, instructions: str) -> Agent[None, str]:
    """Create an agent backed by the deterministic function model."""
    return Agent(
        FunctionModel(_model, model_name="sideseat-fixture"),
        instructions=instructions,
        name=name,
    )


def run(trace_attrs: dict[str, Any], client: Any) -> None:
    """Run text, conversation-history, and tool-use traces in one session."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    with client.trace("pydantic-ai-text", session_id=session_id, user_id=user_id):
        result = _agent(
            name="facts",
            instructions="Answer in one sentence.",
        ).run_sync("What is the speed of light?")
        print(f"Assistant: {result.output}")

    with client.trace("pydantic-ai-history", session_id=session_id, user_id=user_id):
        agent = _agent(
            name="conversation",
            instructions="Answer in one sentence.",
        )
        first = agent.run_sync("What is the boiling point of water?")
        second = agent.run_sync(
            "What is that on the same scale used in the United States?",
            message_history=first.all_messages(),
        )
        print(f"Assistant: {first.output}")
        print(f"Assistant: {second.output}")

    with client.trace("pydantic-ai-tools", session_id=session_id, user_id=user_id):
        agent = _agent(
            name="weather",
            instructions="Use tools when available.",
        )

        @agent.tool_plain
        def get_weather(location: str) -> str:
            """Get the current weather for a city."""
            return f"Sunny, 22°C in {location}"

        result = agent.run_sync("What's the weather in Paris?")
        print(f"Assistant: {result.output}")
