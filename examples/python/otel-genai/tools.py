"""The application's model calls, each recorded with OpenTelemetry's GenAI utilities.

The application talks to the OpenAI Responses API with the official client and records every request
the way the utilities document: one inference invocation per request, holding the system instruction,
the input it sent as semantic-convention message parts, and the output it received. Each Responses item
becomes the part the conventions define for it - text, a tool call and its response, a provider-run
tool call and its response, an inline blob - built from the utilities' own dataclasses, so the shapes
are theirs.
"""

from __future__ import annotations

import json
import os
from collections.abc import Sequence
from typing import Any

from models import OpenAIModel
from opentelemetry.util.genai.handler import TelemetryHandler, get_telemetry_handler
from opentelemetry.util.genai.types import (
    InputMessage,
    MessagePart,
    Modality,
    OutputMessage,
    ServerToolCallPart,
    ServerToolCallResponsePart,
    TextPart,
    ToolCallRequestPart,
    ToolCallResponsePart,
)
from opentelemetry.util.genai.utils import image_from_url

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

_handler: TelemetryHandler | None = None


def handler() -> TelemetryHandler:
    """The utilities' handler, with message content on the spans.

    Content capture is the application's choice and is off by default; the utilities read it once,
    when the handler is made, from ``OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT``.
    """
    global _handler
    if _handler is None:
        os.environ["OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT"] = "SPAN_ONLY"
        _handler = get_telemetry_handler()
    return _handler


def definitions(*names: str) -> list[dict[str, Any]]:
    """Responses API function tools."""
    return [
        {
            "type": "function",
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters,
        }
        for tool in (spec(FUNCTIONS[name]) for name in names)
    ]


def respond(
    model: OpenAIModel,
    items: list[dict[str, Any]],
    *,
    tools: Sequence[dict[str, Any]] = (),
) -> str:
    """Calls the Responses API until the model stops calling functions; returns the final text."""
    while True:
        response = _create(model, items, tools)
        output = [item.model_dump(exclude_none=True) for item in response.output]
        items.extend(output)
        calls = [item for item in output if item["type"] == "function_call"]
        if not calls:
            return str(response.output_text)
        for call in calls:
            outcome = execute(FUNCTIONS, call["name"], call.get("arguments") or "{}")
            items.append(
                {
                    "type": "function_call_output",
                    "call_id": call["call_id"],
                    "output": outcome.text,
                }
            )


def _create(
    model: OpenAIModel, items: list[dict[str, Any]], tools: Sequence[dict[str, Any]]
) -> Any:
    fields: dict[str, Any] = {
        "model": model.id,
        "instructions": content.SYSTEM,
        "input": items,
    }
    if tools:
        fields["tools"] = list(tools)
    with handler().inference("openai", request_model=model.id) as invocation:
        invocation.system_instruction = [TextPart(content=content.SYSTEM)]
        invocation.input_messages = input_messages(items)
        response = model.client.responses.create(**fields)
        output = [item.model_dump(exclude_none=True) for item in response.output]
        invocation.output_messages = [output_message(output)]
        invocation.finish_reasons = [
            "tool_call"
            if any(item["type"] == "function_call" for item in output)
            else "stop"
        ]
        invocation.response_id = response.id
        invocation.response_model_name = response.model
        if response.usage is not None:
            invocation.input_tokens = response.usage.input_tokens
            invocation.output_tokens = response.usage.output_tokens
    return response


def input_messages(items: Sequence[dict[str, Any]]) -> list[InputMessage]:
    """The request's input items as conventional messages, in the order they were sent."""
    messages: list[InputMessage] = []
    for item in items:
        role, parts = _item(item)
        if messages and messages[-1].role == role and role != "user":
            messages[-1].parts.extend(parts)
        else:
            messages.append(InputMessage(role=role, parts=parts))
    return messages


def output_message(output: Sequence[dict[str, Any]]) -> OutputMessage:
    """What the response returned, as one assistant message."""
    parts: list[MessagePart] = []
    for item in output:
        parts.extend(_item(item)[1])
    finish = (
        "tool_call"
        if any(item["type"] == "function_call" for item in output)
        else "stop"
    )
    return OutputMessage(role="assistant", parts=parts, finish_reason=finish)


def _item(item: dict[str, Any]) -> tuple[str, list[MessagePart]]:
    kind = item.get("type", "message")
    if kind == "message":
        return item.get("role", "user"), _content(item.get("content"))
    if kind == "function_call":
        return "assistant", [
            ToolCallRequestPart(
                arguments=json.loads(item.get("arguments") or "{}"),
                name=item["name"],
                id=item.get("call_id"),
            )
        ]
    if kind == "function_call_output":
        return "tool", [
            ToolCallResponsePart(response=item.get("output"), id=item.get("call_id"))
        ]
    if kind == "web_search_call":
        # The provider ran the search: the call is what it searched for, the response what it found.
        action = item.get("action") or {}
        return "assistant", [
            ServerToolCallPart(
                name="web_search",
                id=item.get("id"),
                server_tool_call={"type": "web_search", "action": action},
            ),
            ServerToolCallResponsePart(
                id=item.get("id"),
                server_tool_call_response={
                    "type": "web_search",
                    "status": item.get("status"),
                    "sources": action.get("sources") or [],
                },
            ),
        ]
    raise ValueError(f"no conventional part for a {kind!r} item")


def _content(value: Any) -> list[MessagePart]:
    if isinstance(value, str):
        return [TextPart(content=value)]
    parts: list[MessagePart] = []
    for block in value or []:
        kind = block.get("type")
        if kind in ("input_text", "output_text"):
            parts.append(TextPart(content=block["text"]))
        elif kind == "input_image":
            # A data URL is an inline blob of the image modality.
            image = image_from_url(block["image_url"])
            if image is not None:
                parts.append(image)
        elif kind == "input_file":
            document = image_from_url(block["file_data"], modality=Modality.DOCUMENT)
            if document is not None:
                parts.append(document)
        else:
            raise ValueError(f"no conventional part for a {kind!r} content block")
    return parts
