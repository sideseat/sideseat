"""History, streaming, decorators, tools, traces, and sessions in TraceLoop."""

import json
from typing import Any

from traceloop.sdk.decorators import agent, task, tool, workflow

_client: Any = None
_weather_calls = 0


def _openai() -> Any:
    if _client is None:
        raise RuntimeError("TraceLoop sample client was not initialized")
    return _client


@task(name="retain-history")
def _record_history(first: str, second: str) -> dict[str, str]:
    """Exercise task entity I/O without turning arbitrary values into chat."""
    return {"first": first, "second": second}


@workflow(name="science-history")
def _history(model: str) -> dict[str, str]:
    """Run a retained-history conversation with a streamed second response."""
    messages: list[dict[str, Any]] = [
        {
            "role": "system",
            "content": "Answer scientific questions in one sentence.",
        },
        {"role": "user", "content": "What is the speed of light?"},
    ]
    first_response = _openai().chat.completions.create(
        model=model,
        messages=messages,
    )
    first = first_response.choices[0].message.content or ""
    if not first:
        raise RuntimeError("TraceLoop non-streaming response was empty")

    messages.extend(
        [
            {"role": "assistant", "content": first},
            {"role": "user", "content": "What is the boiling point of water?"},
        ]
    )
    stream = _openai().chat.completions.create(
        model=model,
        messages=messages,
        stream=True,
        stream_options={"include_usage": True},
    )
    second = "".join(
        chunk.choices[0].delta.content or "" for chunk in stream if chunk.choices
    )
    if not second:
        raise RuntimeError("TraceLoop streaming response was empty")
    return _record_history(first, second)


@tool(name="get_weather")
def _get_weather(location: str) -> str:
    """Return deterministic weather for a city."""
    global _weather_calls
    _weather_calls += 1
    return f"Sunny, 22°C, light breeze in {location}."


@agent(name="weather-agent")
def _tool_roundtrip(model: str) -> str:
    """Run one complete OpenAI function-call roundtrip."""
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
    call_response = _openai().chat.completions.create(
        model=model,
        messages=messages,
        tools=tools,
    )
    assistant = call_response.choices[0].message
    if not assistant.tool_calls:
        raise RuntimeError("TraceLoop model did not request the weather tool")

    call = assistant.tool_calls[0]
    arguments = json.loads(call.function.arguments)
    result = _get_weather(**arguments)
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
    final_response = _openai().chat.completions.create(
        model=model,
        messages=messages,
        tools=tools,
    )
    final = final_response.choices[0].message.content or ""
    if not final:
        raise RuntimeError("TraceLoop final tool response was empty")
    return final


def run(client: Any, model: str, trace_attrs: dict[str, str], owner: Any) -> None:
    """Emit two traces that share one session and cover the production rubric."""
    global _client, _weather_calls
    _client = client
    _weather_calls = 0
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    with owner.trace(
        "traceloop-history",
        session_id=session_id,
        user_id=user_id,
    ):
        history = _history(model)
        print(history["second"])

    with owner.trace(
        "traceloop-tool-roundtrip",
        session_id=session_id,
        user_id=user_id,
    ):
        print(_tool_roundtrip(model))

    if _weather_calls != 1:
        raise RuntimeError(f"TraceLoop executed get_weather {_weather_calls} times")
