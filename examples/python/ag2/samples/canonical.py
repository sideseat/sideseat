"""History, streaming, tools, traces, and sessions in one AG2 capture."""

from collections.abc import Callable
from typing import Any

from ag2 import Agent, MemoryStream, tool
from ag2.middleware.builtin import TelemetryMiddleware

_weather_calls = 0


@tool(name="get_weather", description="Get weather for a city")
def get_weather(location: str) -> str:
    """Return deterministic weather for a city."""
    global _weather_calls
    _weather_calls += 1
    return f"Sunny, 22°C, light breeze in {location}."


def _middleware(client: Any, agent_name: str, use_sideseat: bool) -> tuple[Any, ...]:
    """Return explicit native middleware; SideSeat injects the SDK counterpart."""
    if use_sideseat:
        return ()
    return (
        TelemetryMiddleware(
            tracer_provider=client.tracer_provider,
            capture_content=True,
            agent_name=agent_name,
        ),
    )


async def _require_content(reply: Any, description: str) -> str:
    """Return one non-empty AG2 response body."""
    value = await reply.content()
    text = str(value or "").strip()
    if not text:
        raise RuntimeError(f"AG2 {description} response was empty")
    return text


async def run(
    config_factory: Callable[..., Any],
    trace_attrs: dict[str, str],
    client: Any,
    use_sideseat: bool,
) -> None:
    """Emit two traces that share one session and cover the production rubric."""
    global _weather_calls
    _weather_calls = 0
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    history_agent = Agent(
        "HistoryAgent",
        "Answer scientific questions in one sentence.",
        config=config_factory(),
        middleware=_middleware(client, "HistoryAgent", use_sideseat),
    )
    history = MemoryStream()
    with client.trace("ag2-history", session_id=session_id, user_id=user_id):
        first = await history_agent.ask(
            "What is the speed of light?",
            stream=history,
        )
        await _require_content(first, "non-streaming")

        second = await history_agent.ask(
            "What is the boiling point of water?",
            stream=history,
            config=config_factory(streaming=True),
        )
        print(await _require_content(second, "streaming"))

    tool_agent = Agent(
        "ToolAgent",
        "Use tools when they are available.",
        config=config_factory(),
        tools=[get_weather],
        middleware=_middleware(client, "ToolAgent", use_sideseat),
    )
    with client.trace(
        "ag2-tool-roundtrip",
        session_id=session_id,
        user_id=user_id,
    ):
        response = await tool_agent.ask("What is the weather in Paris?")
        print(await _require_content(response, "tool"))

    if _weather_calls != 1:
        raise RuntimeError(f"AG2 executed get_weather {_weather_calls} times")
