"""History, streaming, tools, traces, and sessions through Google Vertex AI."""

from typing import Any

from google.genai import types


def _history(model: Any) -> None:
    """Run a retained-history conversation with a streamed second response."""
    chat = model.client.chats.create(
        model=model.model_id,
        config=types.GenerateContentConfig(
            system_instruction="Answer scientific questions in one sentence."
        ),
    )
    first_response = chat.send_message("What is the speed of light?")
    first = first_response.text or ""
    if not first:
        raise RuntimeError("Vertex AI non-streaming response was empty")

    chunks = chat.send_message_stream("What is the boiling point of water?")
    second = "".join(chunk.text or "" for chunk in chunks)
    if not second:
        raise RuntimeError("Vertex AI streaming response was empty")
    print(second)


def _tool_roundtrip(model: Any) -> None:
    """Run one automatic function-call roundtrip."""
    calls = 0

    def get_weather(location: str) -> str:
        """Get deterministic weather for a city."""
        nonlocal calls
        calls += 1
        return f"Sunny, 22°C, light breeze in {location}."

    chat = model.client.chats.create(
        model=model.model_id,
        config=types.GenerateContentConfig(
            system_instruction="Use tools when they are available.",
            tools=[get_weather],
        ),
    )
    response = chat.send_message("What is the weather in Paris?")
    final = response.text or ""
    if not final:
        raise RuntimeError("Vertex AI final tool response was empty")
    if calls != 1:
        raise RuntimeError(f"Vertex AI executed get_weather {calls} times")
    print(final)


def run(model: Any, trace_attrs: dict[str, str], owner: Any) -> None:
    """Emit two traces that share one session and cover the production rubric."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    with owner.trace(
        "vertex-ai-history",
        session_id=session_id,
        user_id=user_id,
    ):
        _history(model)

    with owner.trace(
        "vertex-ai-tool-roundtrip",
        session_id=session_id,
        user_id=user_id,
    ):
        _tool_roundtrip(model)
