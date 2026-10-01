"""History, streaming, tools, traces, and sessions in one LlamaIndex capture."""

from typing import Any

from llama_index.core.agent.workflow import FunctionAgent
from llama_index.core.agent.workflow.workflow_events import AgentStream
from llama_index.core.workflow import Context


def _text(value: Any) -> str:
    """Return the textual result exposed by an agent workflow."""
    return str(value).strip()


async def run(model: Any, trace_attrs: dict[str, str], client: Any) -> None:
    """Emit two traces that share one session and cover the production rubric."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    history_agent = FunctionAgent(
        name="HistoryAgent",
        llm=model,
        tools=[],
        system_prompt="Answer scientific questions in one sentence.",
        streaming=False,
    )
    history_context = Context(history_agent)
    with client.trace(
        "llamaindex-history",
        session_id=session_id,
        user_id=user_id,
    ):
        first = await history_agent.run(
            user_msg="What is the speed of light?",
            ctx=history_context,
        )
        if not _text(first):
            raise RuntimeError("LlamaIndex non-streaming response was empty")

        history_agent.streaming = True
        handler = history_agent.run(
            user_msg="What is the boiling point of water?",
            ctx=history_context,
        )
        deltas = [
            event.delta
            async for event in handler.stream_events()
            if isinstance(event, AgentStream) and event.delta
        ]
        second = await handler
        if not deltas or not _text(second):
            raise RuntimeError("LlamaIndex streaming response was empty")
        print("".join(deltas))

    tool_calls = 0

    def get_weather(location: str) -> str:
        """Return deterministic weather for a city."""
        nonlocal tool_calls
        tool_calls += 1
        return f"Sunny, 22°C, light breeze in {location}."

    tool_agent = FunctionAgent(
        name="ToolAgent",
        llm=model,
        tools=[get_weather],
        system_prompt="Use tools when they are available.",
        streaming=False,
    )
    with client.trace(
        "llamaindex-tool-roundtrip",
        session_id=session_id,
        user_id=user_id,
    ):
        final = await tool_agent.run(user_msg="What is the weather in Paris?")
        if not _text(final):
            raise RuntimeError("LlamaIndex final tool response was empty")
        if tool_calls != 1:
            raise RuntimeError(f"LlamaIndex executed get_weather {tool_calls} times")
        print(_text(final))
