"""History, streaming, tools, traces, and sessions in one Smolagents capture."""

from typing import Any

from smolagents import Tool, ToolCallingAgent


class WeatherTool(Tool):
    """Deterministic weather tool with an observable execution count."""

    name = "get_weather"
    description = "Return deterministic weather for a city."
    inputs = {
        "location": {
            "type": "string",
            "description": "City to inspect.",
        }
    }
    output_type = "string"
    calls = 0

    def forward(self, location: str) -> str:
        """Return deterministic weather for one city."""
        self.calls += 1
        return f"Sunny, 22°C in {location}."


def _text(value: Any) -> str:
    """Return a non-empty textual result from an agent output."""
    if isinstance(value, str):
        return value
    output = getattr(value, "output", None)
    return output if isinstance(output, str) else ""


def run(model: Any, trace_attrs: dict[str, str], client: Any) -> None:
    """Emit two traces that share one session and cover the production rubric."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    history_agent = ToolCallingAgent(
        tools=[],
        model=model,
        max_steps=2,
        stream_outputs=True,
    )
    with client.trace(
        "smolagents-history",
        session_id=session_id,
        user_id=user_id,
    ):
        first = _text(history_agent.run("What is the speed of light?"))
        if not first:
            raise RuntimeError("Smolagents non-streaming response was empty")

        chunks = list(
            history_agent.run(
                "What is the boiling point of water?",
                reset=False,
                stream=True,
            )
        )
        if not any(
            type(chunk).__name__ == "ChatMessageStreamDelta" for chunk in chunks
        ):
            raise RuntimeError("Smolagents model streaming emitted no deltas")
        streamed = _text(chunks[-1]) if chunks else ""
        if not streamed:
            raise RuntimeError("Smolagents streaming response was empty")
        print(streamed)

    weather_tool = WeatherTool()
    tool_agent = ToolCallingAgent(
        tools=[weather_tool],
        model=model,
        max_steps=3,
    )
    with client.trace(
        "smolagents-tool-roundtrip",
        session_id=session_id,
        user_id=user_id,
    ):
        final_answer = _text(tool_agent.run("What is the weather in Paris?"))
        if not final_answer:
            raise RuntimeError("Smolagents final tool response was empty")
        if weather_tool.calls != 1:
            raise RuntimeError(
                f"Smolagents executed get_weather {weather_tool.calls} times"
            )
        print(final_answer)
