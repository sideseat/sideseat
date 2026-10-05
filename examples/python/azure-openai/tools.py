"""The shared tools as Chat Completions functions, and the application side of the tool loop.

The loop is stateless: each request re-sends the whole conversation, the system prompt first.
"""

from collections.abc import Sequence
from typing import Any

from models import Deployment

from harness import content
from harness.tooling import execute, spec

FUNCTIONS = {
    function.__name__: function
    for function in (
        content.get_weather,
        content.get_precipitation,
        content.book_flight,
    )
}


def definitions(*names: str) -> list[dict[str, Any]]:
    return [
        {
            "type": "function",
            "function": {
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            },
        }
        for tool in (spec(FUNCTIONS[name]) for name in names)
    ]


def system() -> dict[str, str]:
    return {"role": "system", "content": content.SYSTEM}


def converse(
    deployment: Deployment,
    messages: list[dict[str, Any]],
    *,
    tools: Sequence[dict[str, Any]] = (),
    stream: bool = False,
    **request: Any,
) -> str:
    """Calls the deployment until it stops calling tools; returns the final text."""
    if tools:
        request["tools"] = list(tools)
    while True:
        create = _stream if stream else _create
        message = create(deployment, messages, request)
        messages.append(message)
        calls = message.get("tool_calls") or []
        if not calls:
            return str(message.get("content") or "")
        for call in calls:
            function = call["function"]
            outcome = execute(FUNCTIONS, function["name"], function["arguments"])
            messages.append(
                {"role": "tool", "tool_call_id": call["id"], "content": outcome.text}
            )


def _create(
    deployment: Deployment, messages: list[dict[str, Any]], request: dict[str, Any]
) -> dict[str, Any]:
    response = deployment.client.chat.completions.create(
        model=deployment.id, messages=messages, **request
    )
    # A plain dictionary, as an application stores history.
    return response.choices[0].message.model_dump(exclude_none=True)


def _stream(
    deployment: Deployment, messages: list[dict[str, Any]], request: dict[str, Any]
) -> dict[str, Any]:
    text: list[str] = []
    calls: dict[int, dict[str, Any]] = {}
    chunks = deployment.client.chat.completions.create(
        model=deployment.id,
        messages=messages,
        stream=True,
        stream_options={"include_usage": True},
        **request,
    )
    for chunk in chunks:
        if not chunk.choices:
            continue
        delta = chunk.choices[0].delta
        if delta.content:
            print(delta.content, end="", flush=True)
            text.append(delta.content)
        for piece in delta.tool_calls or []:
            call = calls.setdefault(
                piece.index,
                {
                    "id": "",
                    "type": "function",
                    "function": {"name": "", "arguments": ""},
                },
            )
            call["id"] = piece.id or call["id"]
            if piece.function is not None:
                call["function"]["name"] += piece.function.name or ""
                call["function"]["arguments"] += piece.function.arguments or ""
    print()
    message: dict[str, Any] = {"role": "assistant", "content": "".join(text) or None}
    if calls:
        message["tool_calls"] = [calls[index] for index in sorted(calls)]
    return message
