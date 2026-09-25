"""Google GenAI sync, streaming, and automatic tool-calling scenarios."""

from typing import Any

from google.genai import types


def run(model: Any, trace_attrs: dict[str, str], client: Any) -> None:
    """Run independent text calls and one session-scoped tool roundtrip."""
    response = model.client.models.generate_content(
        model=model.model_id,
        contents="What is the speed of light?",
        config=types.GenerateContentConfig(
            system_instruction="Answer in one sentence."
        ),
    )
    print(f"Assistant: {response.text}")

    chunks = model.client.models.generate_content_stream(
        model=model.model_id,
        contents="What is the boiling point of water?",
        config=types.GenerateContentConfig(
            system_instruction="Answer in one sentence."
        ),
    )
    streamed = "".join(chunk.text or "" for chunk in chunks)
    print(f"Assistant: {streamed}")

    with client.trace(
        "google-genai-tool-use",
        session_id=trace_attrs["session.id"],
        user_id=trace_attrs["user.id"],
    ):

        def get_weather(location: str) -> str:
            """Get the current weather for a city."""
            return f"Sunny, 22°C in {location}"

        chat = model.client.chats.create(
            model=model.model_id,
            config=types.GenerateContentConfig(
                system_instruction="Use tools when available.",
                tools=[get_weather],
            ),
        )
        response = chat.send_message("What's the weather in Paris?")
        print(f"Assistant: {response.text}")
