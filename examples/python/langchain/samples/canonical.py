"""History, streaming, tools, traces, and sessions in one LangChain capture."""

from typing import Any

from langchain_core.messages import (
    BaseMessage,
    HumanMessage,
    SystemMessage,
    ToolMessage,
)
from langchain_core.tools import tool


@tool
def get_weather(location: str) -> str:
    """Return deterministic weather for a city."""
    return f"Sunny, 22C, light breeze in {location}"


def _text(content: Any) -> str:
    """Render text content returned by the chat model."""
    if isinstance(content, str):
        return content
    return str(content)


def run(model: Any, trace_attrs: dict[str, str], client: Any) -> None:
    """Emit two traces that share one session and cover the production rubric."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    with client.trace(
        "langchain-history",
        session_id=session_id,
        user_id=user_id,
    ):
        history: list[BaseMessage] = [
            SystemMessage(content="Answer scientific questions in one sentence."),
            HumanMessage(content="What is the speed of light?"),
        ]
        first = model.invoke(history)
        history.extend(
            [
                first,
                HumanMessage(content="What is the boiling point of water?"),
            ]
        )
        streamed = "".join(
            _text(chunk.content) for chunk in model.stream(history) if chunk.content
        )
        if not streamed:
            raise RuntimeError("LangChain streaming response was empty")
        print(streamed)

    with client.trace(
        "langchain-tool-roundtrip",
        session_id=session_id,
        user_id=user_id,
    ):
        tool_model = model.bind_tools([get_weather])
        messages: list[BaseMessage] = [
            SystemMessage(content="Use tools when they are available."),
            HumanMessage(content="What is the weather in Paris?"),
        ]
        call_message = tool_model.invoke(messages)
        messages.append(call_message)
        if not call_message.tool_calls:
            raise RuntimeError("LangChain model did not request the weather tool")

        call = call_message.tool_calls[0]
        result = get_weather.invoke(call["args"])
        messages.append(
            ToolMessage(
                content=result,
                tool_call_id=call["id"],
                name=call["name"],
            )
        )
        final = tool_model.invoke(messages)
        if not _text(final.content):
            raise RuntimeError("LangChain final tool response was empty")
        print(_text(final.content))
