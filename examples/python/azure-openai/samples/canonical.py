"""History, streaming, tools, traces, and sessions through Azure OpenAI v1."""

import json
from typing import Any


def _history(client: Any, model: str) -> None:
    messages: list[dict[str, Any]] = [
        {
            "role": "system",
            "content": "Answer scientific questions in one sentence.",
        },
        {"role": "user", "content": "What is the speed of light?"},
    ]
    first_response = client.chat.completions.create(model=model, messages=messages)
    first = first_response.choices[0].message.content or ""
    if not first:
        raise RuntimeError("Azure OpenAI non-streaming response was empty")

    messages.extend(
        [
            {"role": "assistant", "content": first},
            {"role": "user", "content": "What is the boiling point of water?"},
        ]
    )
    stream = client.chat.completions.create(
        model=model,
        messages=messages,
        stream=True,
        stream_options={"include_usage": True},
    )
    second = "".join(
        chunk.choices[0].delta.content or "" for chunk in stream if chunk.choices
    )
    if not second:
        raise RuntimeError("Azure OpenAI streaming response was empty")
    print(second)


def _tool_roundtrip(client: Any, model: str) -> None:
    tools = [
        {
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get deterministic weather for a city.",
                "parameters": {
                    "type": "object",
                    "properties": {"location": {"type": "string"}},
                    "required": ["location"],
                },
            },
        }
    ]
    messages: list[dict[str, Any]] = [
        {"role": "system", "content": "Use tools when they are available."},
        {"role": "user", "content": "What is the weather in Paris?"},
    ]
    call_response = client.chat.completions.create(
        model=model,
        messages=messages,
        tools=tools,
    )
    assistant = call_response.choices[0].message
    if not assistant.tool_calls:
        raise RuntimeError("Azure OpenAI model did not request the weather tool")

    call = assistant.tool_calls[0]
    arguments = json.loads(call.function.arguments)
    result = f"Sunny, 22°C, light breeze in {arguments['location']}."
    messages.extend(
        [
            assistant.model_dump(exclude_none=True),
            {
                "role": "tool",
                "tool_call_id": call.id,
                "name": call.function.name,
                "content": result,
            },
        ]
    )
    final_response = client.chat.completions.create(
        model=model,
        messages=messages,
        tools=tools,
    )
    final = final_response.choices[0].message.content or ""
    if not final:
        raise RuntimeError("Azure OpenAI final tool response was empty")
    print(final)


def run(client: Any, model: str, trace_attrs: dict[str, str], owner: Any) -> None:
    """Emit two traces that share one session and cover the production rubric."""
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    with owner.trace(
        "azure-openai-history",
        session_id=session_id,
        user_id=user_id,
    ):
        _history(client, model)

    with owner.trace(
        "azure-openai-tool-roundtrip",
        session_id=session_id,
        user_id=user_id,
    ):
        _tool_roundtrip(client, model)
