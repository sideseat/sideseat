#!/usr/bin/env python3
"""Deterministic local OpenAI-compatible endpoint for framework fixture capture."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


def usage() -> dict[str, int]:
    return {
        "prompt_tokens": 12,
        "completion_tokens": 7,
        "total_tokens": 19,
    }


def responses_usage() -> dict[str, Any]:
    return {
        "input_tokens": 12,
        "input_tokens_details": {
            "cache_write_tokens": 0,
            "cached_tokens": 0,
        },
        "output_tokens": 7,
        "output_tokens_details": {"reasoning_tokens": 0},
        "total_tokens": 19,
    }


def request_fingerprint(value: Any) -> str:
    """Return a stable, request-specific suffix for provider object IDs."""
    payload = json.dumps(
        value,
        ensure_ascii=True,
        separators=(",", ":"),
        sort_keys=True,
    ).encode()
    return hashlib.sha256(payload).hexdigest()[:12]


def tool_definition(body: dict[str, Any]) -> tuple[str, dict[str, Any]] | None:
    for tool in body.get("tools", []):
        function = tool.get("function", tool)
        name = function.get("name")
        if isinstance(name, str) and name:
            parameters = function.get("parameters", {})
            return name, parameters if isinstance(parameters, dict) else {}
    return None


def named_tool_definition(
    body: dict[str, Any], wanted: str
) -> tuple[str, dict[str, Any]] | None:
    """Return one declared tool by name."""
    for tool in body.get("tools", []):
        function = tool.get("function", tool)
        if function.get("name") != wanted:
            continue
        parameters = function.get("parameters", {})
        return wanted, parameters if isinstance(parameters, dict) else {}
    return None


def property_value(name: str, schema: dict[str, Any], context: str = "") -> Any:
    if name == "answer":
        lowered = context.lower()
        if "boiling point" in lowered:
            return "Water boils at 100°C at sea level."
        if "speed of light" in lowered:
            return "The speed of light is 299,792,458 metres per second."

    if name in {"file_path", "image_path", "path"}:
        path = re.search(
            r"(?:/[^\s'\",]+|[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)+)"
            r"\.(?:gif|jpe?g|pdf|png|webp)",
            context,
            re.IGNORECASE,
        )
        if path:
            return path.group(0)

    known: dict[str, Any] = {
        "city": "Paris",
        "expression": "12345 + 6789",
        "file_path": "examples/assets/img.jpg",
        "location": "Paris",
        "path": "examples/assets/img.jpg",
        "prompt": "A deterministic blue circle on a white background",
        "query": "deterministic local fixture",
        "search_query": "deterministic local fixture",
        "text": "deterministic local fixture",
    }
    if name in known:
        return known[name]
    if schema.get("enum"):
        return schema["enum"][0]
    if "const" in schema:
        return schema["const"]
    value_type = schema.get("type")
    if value_type == "integer":
        return 1
    if value_type == "number":
        return 1.0
    if value_type == "boolean":
        return True
    if value_type == "array":
        return []
    if value_type == "object":
        return {}
    return f"deterministic-{name.replace('_', '-')}"


def tool_arguments(parameters: dict[str, Any], context: str = "") -> str:
    properties = parameters.get("properties", {})
    if not isinstance(properties, dict):
        return "{}"
    required = parameters.get("required", [])
    names = (
        required if isinstance(required, list) and required else list(properties)[:1]
    )
    arguments = {
        name: property_value(name, properties.get(name, {}), context)
        for name in names
        if isinstance(name, str)
    }
    return json.dumps(arguments, separators=(",", ":"))


def completed_tool_name(messages: list[dict[str, Any]]) -> str | None:
    for message in reversed(messages):
        for call in message.get("tool_calls", []) or []:
            function = call.get("function", {})
            name = function.get("name")
            if isinstance(name, str):
                return name
    return None


def current_turn_has_tool_result(messages: list[dict[str, Any]]) -> bool:
    last_tool_call = max(
        (
            index
            for index, message in enumerate(messages)
            if message.get("role") == "assistant" and message.get("tool_calls")
        ),
        default=-1,
    )
    last_tool_result = max(
        (
            index
            for index, message in enumerate(messages)
            if message.get("role") == "tool"
            or observation_result_text(message) is not None
        ),
        default=-1,
    )
    last_final_assistant = max(
        (
            index
            for index, message in enumerate(messages)
            if message.get("role") == "assistant"
            and not message.get("tool_calls")
            and message.get("content") is not None
        ),
        default=-1,
    )
    # Strands can append a user-role multimodal block after a tool result so the model can inspect
    # an image. That block still belongs to the active tool turn. A result becomes stale only once
    # the assistant has produced a final response after it.
    return last_tool_result > last_tool_call and last_tool_result > last_final_assistant


def observation_result_text(message: dict[str, Any]) -> str | None:
    """Read an Action/Observation protocol result carried in a user message."""
    if message.get("role") != "user":
        return None
    content = message.get("content")
    if isinstance(content, list):
        content = "\n".join(
            item.get("text", "")
            for item in content
            if isinstance(item, dict) and isinstance(item.get("text"), str)
        )
    if not isinstance(content, str):
        return None
    prefix = "Observation:\n"
    if not content.startswith(prefix):
        return None
    result = content.removeprefix(prefix).strip()
    # Smolagents retains the previous final-answer observation by folding it into the
    # next user turn before a `New task:` marker. That message starts like a tool result,
    # but the active turn is the task after the marker.
    if "\nNew task:\n" in result:
        return None
    return result or None


def latest_tool_result_text(messages: list[dict[str, Any]]) -> str | None:
    for message in reversed(messages):
        if observation := observation_result_text(message):
            return observation
        if message.get("role") != "tool":
            continue
        content = message.get("content")
        if isinstance(content, str) and content:
            return content
        if isinstance(content, list):
            texts = [
                item.get("text")
                for item in content
                if isinstance(item, dict) and isinstance(item.get("text"), str)
            ]
            if texts:
                return "\n".join(texts)
    return None


def referenced_schema(
    schema: dict[str, Any], root_schema: dict[str, Any]
) -> dict[str, Any]:
    reference = schema.get("$ref")
    if not isinstance(reference, str) or not reference.startswith("#/"):
        return schema
    value: Any = root_schema
    for part in reference[2:].split("/"):
        if not isinstance(value, dict):
            return schema
        value = value.get(part.replace("~1", "/").replace("~0", "~"))
    return value if isinstance(value, dict) else schema


def structured_value(
    schema: dict[str, Any],
    name: str = "value",
    root_schema: dict[str, Any] | None = None,
) -> Any:
    if root_schema is None:
        root_schema = schema
    schema = referenced_schema(schema, root_schema)
    if "const" in schema:
        return schema["const"]
    if schema.get("enum"):
        return schema["enum"][0]

    for variant_key in ("anyOf", "oneOf"):
        variants = schema.get(variant_key)
        if isinstance(variants, list):
            non_null = [
                variant
                for variant in variants
                if isinstance(variant, dict) and variant.get("type") != "null"
            ]
            if non_null:
                return structured_value(non_null[0], name, root_schema)
            return None

    value_type = schema.get("type")
    if isinstance(value_type, list):
        value_type = next((item for item in value_type if item != "null"), "null")
    if value_type == "object" or "properties" in schema:
        properties = schema.get("properties", {})
        if not isinstance(properties, dict):
            return {}
        return {
            property_name: structured_value(property_schema, property_name, root_schema)
            for property_name, property_schema in properties.items()
            if isinstance(property_schema, dict)
        }
    if value_type == "array":
        item_schema = schema.get("items", {})
        return (
            [structured_value(item_schema, name, root_schema)]
            if isinstance(item_schema, dict)
            else []
        )
    return property_value(name, schema)


def response_format_schema(body: dict[str, Any]) -> dict[str, Any] | None:
    text = body.get("text", {})
    if not isinstance(text, dict):
        return None
    response_format = text.get("format", {})
    if not isinstance(response_format, dict):
        return None
    schema = response_format.get("schema")
    return schema if isinstance(schema, dict) else None


def chat_response_format_schema(body: dict[str, Any]) -> dict[str, Any] | None:
    response_format = body.get("response_format", {})
    if not isinstance(response_format, dict):
        return None
    json_schema = response_format.get("json_schema", {})
    if not isinstance(json_schema, dict):
        return None
    schema = json_schema.get("schema")
    return schema if isinstance(schema, dict) else None


def response_input(body: dict[str, Any]) -> list[dict[str, Any]]:
    input_value = body.get("input", [])
    if not isinstance(input_value, list):
        return []
    return [item for item in input_value if isinstance(item, dict)]


def completed_response_tool_name(items: list[dict[str, Any]]) -> str | None:
    calls = {
        item.get("call_id"): item.get("name")
        for item in items
        if item.get("type") == "function_call"
    }
    for item in reversed(items):
        if item.get("type") == "function_call_output":
            name = calls.get(item.get("call_id"))
            if isinstance(name, str):
                return name
            call_id = item.get("call_id")
            if isinstance(call_id, str) and call_id.startswith("call-"):
                stem = call_id.removeprefix("call-")
                name, separator, _suffix = stem.rpartition("-")
                return (name if separator else stem).replace("-", "_")
            return None
    return None


def response_text(body: dict[str, Any], has_tool_result: bool) -> str:
    schema = response_format_schema(body)
    if schema is not None:
        return json.dumps(structured_value(schema), separators=(",", ":"))

    items = response_input(body)
    if has_tool_result:
        tool_name = completed_response_tool_name(items)
        if tool_name in {"get_weather", "temperature_forecast"}:
            return "It is sunny and 22°C in Paris."
        if tool_name == "calculate":
            return "The result is 19134."
        return "The tool completed successfully."

    text = " ".join(
        str(content.get("text", ""))
        for item in items
        if item.get("role") == "user"
        for content in item.get("content", [])
        if isinstance(content, dict)
    ).lower()
    if "boiling point" in text:
        return "Water boils at 100°C at sea level."
    if "speed of light" in text:
        return "The speed of light is 299,792,458 metres per second."
    return "This is a deterministic local response."


def response(body: dict[str, Any]) -> dict[str, Any]:
    model = body.get("model", "sideseat-local")
    items = response_input(body)
    has_tool_result = any(item.get("type") == "function_call_output" for item in items)
    declared_tool = tool_definition(body)

    if declared_tool and not has_tool_result:
        tool_name, parameters = declared_tool
        suffix = request_fingerprint(items)
        output = [
            {
                "arguments": tool_arguments(parameters),
                "call_id": f"call-{tool_name.replace('_', '-')}-{suffix}",
                "id": f"fc-{tool_name.replace('_', '-')}-{suffix}",
                "name": tool_name,
                "status": "completed",
                "type": "function_call",
            }
        ]
    else:
        output = [
            {
                "content": [
                    {
                        "annotations": [],
                        "text": response_text(body, has_tool_result),
                        "type": "output_text",
                    }
                ],
                "id": "msg-sideseat-local",
                "role": "assistant",
                "status": "completed",
                "type": "message",
            }
        ]

    created_at = 1_800_000_000.0
    return {
        "completed_at": created_at,
        "created_at": created_at,
        "error": None,
        "id": "resp-sideseat-local",
        "incomplete_details": None,
        "instructions": body.get("instructions"),
        "metadata": body.get("metadata") or {},
        "model": model,
        "object": "response",
        "output": output,
        "parallel_tool_calls": body.get("parallel_tool_calls", True),
        "status": "completed",
        "temperature": body.get("temperature"),
        "tool_choice": body.get("tool_choice", "auto"),
        "tools": body.get("tools", []),
        "top_p": body.get("top_p"),
        "usage": responses_usage(),
    }


def completion(body: dict[str, Any]) -> dict[str, Any]:
    messages = body.get("messages", [])
    model = body.get("model", "sideseat-local")
    has_tool_result = current_turn_has_tool_result(messages)
    declared_tool = tool_definition(body)
    forced_arguments: str | None = None
    observation = latest_tool_result_text(messages) if has_tool_result else None
    if observation and (final_answer := named_tool_definition(body, "final_answer")):
        completed = completed_tool_name(messages)
        if completed in {"get_weather", "temperature_forecast"}:
            answer = "It is sunny and 22°C in Paris."
        elif completed == "calculate":
            answer = "The result is 19134."
        else:
            answer = observation
        declared_tool = final_answer
        forced_arguments = json.dumps({"answer": answer}, separators=(",", ":"))
    if (
        declared_tool
        and declared_tool[0] == "transfer_to_agent"
        and sum(message.get("role") == "user" for message in messages) > 1
    ):
        declared_tool = None

    if declared_tool and (not has_tool_result or forced_arguments is not None):
        tool_name, parameters = declared_tool
        suffix = request_fingerprint(messages)
        message: dict[str, Any] = {
            "role": "assistant",
            "content": None,
            "tool_calls": [
                {
                    "id": f"call-{tool_name.replace('_', '-')}-{suffix}",
                    "type": "function",
                    "function": {
                        "name": tool_name,
                        "arguments": forced_arguments
                        or tool_arguments(
                            parameters,
                            json.dumps(messages, separators=(",", ":")),
                        ),
                    },
                }
            ],
        }
        finish_reason = "tool_calls"
    else:
        schema = chat_response_format_schema(body)
        last_user = next(
            (
                message.get("content", "")
                for message in reversed(messages)
                if message.get("role") == "user"
            ),
            "",
        )
        if schema is not None:
            text = json.dumps(structured_value(schema), separators=(",", ":"))
        elif has_tool_result:
            tool_name = completed_tool_name(messages)
            if tool_name == "get_weather":
                text = "It is sunny and 22°C in Paris."
            elif tool_name == "calculate":
                text = "The result is 19134."
            elif tool_name == "generate_image":
                text = latest_tool_result_text(messages) or "The image was generated."
            else:
                text = "The tool completed successfully."
        elif "boiling point" in str(last_user).lower():
            text = "Water boils at 100°C at sea level."
        elif "speed of light" in str(last_user).lower():
            text = "The speed of light is 299,792,458 metres per second."
        else:
            text = "This is a deterministic local response."
        message = {"role": "assistant", "content": text}
        finish_reason = "stop"

    return {
        "id": "chatcmpl-sideseat-local",
        "object": "chat.completion",
        "created": 1_800_000_000,
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": message,
                "finish_reason": finish_reason,
            }
        ],
        "usage": usage(),
    }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format: str, *args: object) -> None:
        print(f"[fake-openai] {format % args}", flush=True)

    def send_json(self, status: int, value: dict[str, Any]) -> None:
        payload = json.dumps(value, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_request_shape(self, path: str, body: dict[str, Any]) -> None:
        if path == "/v1/chat/completions":
            roles = [
                message.get("role")
                for message in body.get("messages", [])
                if isinstance(message, dict)
            ]
        else:
            roles = [
                item.get("type") or item.get("role") for item in response_input(body)
            ]
        if len(roles) > 8:
            roles = [*roles[:3], f"...({len(roles)})", *roles[-3:]]
        declared_tool = tool_definition(body)
        print(
            "[fake-openai] request"
            f" path={path} stream={bool(body.get('stream'))}"
            f" roles={roles} tool={declared_tool[0] if declared_tool else '-'}"
            f" structured={bool(response_format_schema(body) or chat_response_format_schema(body))}",
            flush=True,
        )

    def do_GET(self) -> None:
        if self.path == "/health":
            self.send_json(200, {"status": "ok"})
            return
        if self.path == "/v1/files/file-sideseat-local/content":
            payload = b"deterministic local file"
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        self.send_json(404, {"error": {"message": "not found", "type": "not_found"}})

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        raw_body = self.rfile.read(length)
        path = self.path.rstrip("/")
        if path == "/v1/files":
            self.send_json(
                200,
                {
                    "bytes": len(raw_body),
                    "created_at": 1_800_000_000,
                    "expires_at": None,
                    "filename": "fixture.pdf",
                    "id": "file-sideseat-local",
                    "object": "file",
                    "purpose": "user_data",
                    "status": "processed",
                    "status_details": None,
                },
            )
            return
        body = json.loads(raw_body or b"{}")
        if path not in {"/v1/chat/completions", "/v1/responses"}:
            self.send_json(
                404,
                {"error": {"message": "unsupported endpoint", "type": "not_found"}},
            )
            return
        self.log_request_shape(path, body)
        if body.get("model") == "nonexistent-model-id-12345":
            self.send_json(
                404,
                {
                    "error": {
                        "message": "model not found",
                        "type": "invalid_request_error",
                    }
                },
            )
            return

        if path == "/v1/responses":
            if body.get("stream"):
                self.send_json(
                    400,
                    {
                        "error": {
                            "message": "streaming Responses API is not supported",
                            "type": "invalid_request_error",
                        }
                    },
                )
                return
            self.send_json(200, response(body))
            return

        value = completion(body)
        if not body.get("stream"):
            self.send_json(200, value)
            return

        choice = value["choices"][0]
        message = choice["message"]
        events = [
            {
                "id": value["id"],
                "object": "chat.completion.chunk",
                "created": value["created"],
                "model": value["model"],
                "choices": [
                    {
                        "index": 0,
                        "delta": {"role": "assistant", "content": ""},
                        "finish_reason": None,
                    }
                ],
            },
        ]
        if tool_calls := message.get("tool_calls"):
            events.append(
                {
                    "id": value["id"],
                    "object": "chat.completion.chunk",
                    "created": value["created"],
                    "model": value["model"],
                    "choices": [
                        {
                            "index": 0,
                            "delta": {
                                "tool_calls": [
                                    {"index": index, **tool_call}
                                    for index, tool_call in enumerate(tool_calls)
                                ]
                            },
                            "finish_reason": None,
                        }
                    ],
                }
            )
        else:
            events.append(
                {
                    "id": value["id"],
                    "object": "chat.completion.chunk",
                    "created": value["created"],
                    "model": value["model"],
                    "choices": [
                        {
                            "index": 0,
                            "delta": {"content": message.get("content", "")},
                            "finish_reason": None,
                        }
                    ],
                }
            )
        events.append(
            {
                "id": value["id"],
                "object": "chat.completion.chunk",
                "created": value["created"],
                "model": value["model"],
                "choices": [
                    {
                        "index": 0,
                        "delta": {},
                        "finish_reason": choice["finish_reason"],
                    }
                ],
                "usage": usage(),
            }
        )
        payload = "".join(
            f"data: {json.dumps(event, separators=(',', ':'))}\n\n" for event in events
        )
        payload += "data: [DONE]\n\n"
        encoded = payload.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)
        self.wfile.flush()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=5401)
    args = parser.parse_args()

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"[fake-openai] listening on http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
