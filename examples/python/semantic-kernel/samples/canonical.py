"""History, streaming, tools, traces, and sessions in one Semantic Kernel capture."""

from collections.abc import Callable
from typing import Annotated, Any

from semantic_kernel.agents import ChatCompletionAgent
from semantic_kernel.functions import kernel_function


def _text(value: Any) -> str:
    """Return one non-empty message as text."""
    return str(value).strip()


class WeatherPlugin:
    """Deterministic tool used by the canonical agent."""

    def __init__(self) -> None:
        self.calls = 0

    @kernel_function(name="get_weather", description="Get weather for a city")
    def get_weather(self, location: Annotated[str, "City to inspect"]) -> str:
        """Return deterministic weather for a city."""
        self.calls += 1
        return f"Sunny, 22°C, light breeze in {location}."


async def run(
    service_factory: Callable[[], Any],
    trace_attrs: dict[str, str],
    client: Any,
) -> None:
    """Emit two traces that share one session and cover the production rubric."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    history_agent = ChatCompletionAgent(
        service=service_factory(),
        name="HistoryAgent",
        instructions="Answer scientific questions in one sentence.",
    )
    with client.trace(
        "semantic-kernel-history",
        session_id=session_id,
        user_id=user_id,
    ):
        first = await history_agent.get_response("What is the speed of light?")
        if not _text(first.message):
            raise RuntimeError("Semantic Kernel non-streaming response was empty")

        chunks: list[str] = []
        async for item in history_agent.invoke_stream(
            "What is the boiling point of water?",
            thread=first.thread,
        ):
            text = _text(item.message)
            if text:
                chunks.append(text)
        if not chunks:
            raise RuntimeError("Semantic Kernel streaming response was empty")
        print("".join(chunks))

    weather = WeatherPlugin()
    tool_agent = ChatCompletionAgent(
        service=service_factory(),
        name="ToolAgent",
        instructions="Use tools when they are available.",
        plugins=[weather],
    )
    with client.trace(
        "semantic-kernel-tool-roundtrip",
        session_id=session_id,
        user_id=user_id,
    ):
        response = await tool_agent.get_response("What is the weather in Paris?")
        if not _text(response.message):
            raise RuntimeError("Semantic Kernel final tool response was empty")
        if weather.calls != 1:
            raise RuntimeError(
                f"Semantic Kernel executed get_weather {weather.calls} times"
            )
        print(_text(response.message))
