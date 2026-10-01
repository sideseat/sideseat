"""History, streaming, tools, traces, and sessions in one Agno capture."""

from typing import Any

from agno.agent import Agent
from agno.db.in_memory import InMemoryDb


def get_weather(location: str) -> str:
    """Return deterministic weather for a city."""
    return f"Sunny, 22°C, light breeze in {location}."


def _content(response: Any) -> str:
    """Return non-empty textual content from an Agno run output."""
    content = getattr(response, "content", None)
    if isinstance(content, str):
        return content
    return "" if content is None else str(content)


def run(model: Any, trace_attrs: dict[str, str], client: Any) -> None:
    """Emit two traces that share one session and cover the production rubric."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    history_agent = Agent(
        id="sideseat-history-agent",
        name="HistoryAgent",
        model=model,
        db=InMemoryDb(),
        add_history_to_context=True,
        system_message="Answer scientific questions in one sentence.",
    )
    with client.trace(
        "agno-history",
        session_id=session_id,
        user_id=user_id,
    ):
        first = history_agent.run(
            "What is the speed of light?",
            session_id=session_id,
            user_id=user_id,
        )
        if not _content(first):
            raise RuntimeError("Agno non-streaming response was empty")

        streamed = "".join(
            _content(chunk)
            for chunk in history_agent.run(
                "What is the boiling point of water?",
                stream=True,
                session_id=session_id,
                user_id=user_id,
            )
        )
        if not streamed:
            raise RuntimeError("Agno streaming response was empty")
        print(streamed)

    tool_agent = Agent(
        id="sideseat-tool-agent",
        name="ToolAgent",
        model=model,
        tools=[get_weather],
        system_message="Use tools when they are available.",
    )
    with client.trace(
        "agno-tool-roundtrip",
        session_id=session_id,
        user_id=user_id,
    ):
        response = tool_agent.run(
            "What is the weather in Paris?",
            session_id=session_id,
            user_id=user_id,
        )
        if not _content(response):
            raise RuntimeError("Agno final tool response was empty")
        print(_content(response))
