"""History, streaming, tools, traces, and sessions in one Haystack capture."""

from collections.abc import Callable
from typing import Annotated, Any

from haystack import Pipeline
from haystack.components.agents import Agent
from haystack.dataclasses import ChatMessage
from haystack.tools import create_tool_from_function


def _pipeline(agent: Agent) -> Pipeline:
    """Build the smallest real Haystack pipeline around an agent."""
    pipeline = Pipeline()
    pipeline.add_component("agent", agent)
    return pipeline


def _text(message: ChatMessage) -> str:
    """Return one generated text message."""
    return (message.text or "").strip()


def run(
    model_factory: Callable[[], Any],
    trace_attrs: dict[str, str],
    client: Any,
) -> None:
    """Emit two traces that share one session and cover the production rubric."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    history_agent = Agent(
        chat_generator=model_factory(),
        system_prompt="Answer scientific questions in one sentence.",
    )
    history_pipeline = _pipeline(history_agent)
    with client.trace(
        "haystack-history",
        session_id=session_id,
        user_id=user_id,
    ):
        first = history_pipeline.run(
            {
                "agent": {
                    "messages": [
                        ChatMessage.from_user("What is the speed of light?"),
                    ]
                }
            }
        )["agent"]
        if not _text(first["last_message"]):
            raise RuntimeError("Haystack non-streaming response was empty")

        prior_messages = [
            message for message in first["messages"] if message.role.value != "system"
        ]
        deltas: list[str] = []

        def collect_chunk(chunk: Any) -> None:
            if isinstance(chunk.content, str) and chunk.content:
                deltas.append(chunk.content)

        history_agent.streaming_callback = collect_chunk
        second = history_pipeline.run(
            {
                "agent": {
                    "messages": [
                        *prior_messages,
                        ChatMessage.from_user("What is the boiling point of water?"),
                    ]
                }
            }
        )["agent"]
        if not deltas or not _text(second["last_message"]):
            raise RuntimeError("Haystack streaming response was empty")
        print("".join(deltas))

    tool_calls = 0

    def get_weather(location: Annotated[str, "City to inspect"]) -> str:
        """Return deterministic weather for a city."""
        nonlocal tool_calls
        tool_calls += 1
        return f"Sunny, 22°C, light breeze in {location}."

    tool_agent = Agent(
        chat_generator=model_factory(),
        tools=[create_tool_from_function(get_weather)],
        system_prompt="Use tools when they are available.",
    )
    tool_pipeline = _pipeline(tool_agent)
    with client.trace(
        "haystack-tool-roundtrip",
        session_id=session_id,
        user_id=user_id,
    ):
        result = tool_pipeline.run(
            {
                "agent": {
                    "messages": [
                        ChatMessage.from_user("What is the weather in Paris?"),
                    ]
                }
            }
        )["agent"]
        if not _text(result["last_message"]):
            raise RuntimeError("Haystack final tool response was empty")
        if tool_calls != 1:
            raise RuntimeError(f"Haystack executed get_weather {tool_calls} times")
        print(_text(result["last_message"]))
