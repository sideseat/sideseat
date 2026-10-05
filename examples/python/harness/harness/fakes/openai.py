"""A local OpenAI endpoint: Chat Completions and Responses, streamed or not, on OpenAI and Azure routes.

Answers come from :mod:`harness.fakes.script`. Azure OpenAI's v1 routes (``/openai/v1/...``),
deployment routes (``/openai/deployments/<name>/chat/completions``), and the ``AzureOpenAI``
client's Responses route (``/openai/responses``) map onto the same handlers.

    uv run --locked --directory examples/python/harness python -m harness.fakes.openai
"""

from __future__ import annotations

import argparse
import json
import re
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import urlsplit

from harness.fakes import script

PORT = 5401
CREATED = 1_800_000_000


def canonical_path(raw: str) -> str:
    path = urlsplit(raw).path.rstrip("/") or "/"
    if path.startswith("/openai/v1/"):
        return "/v1/" + path.removeprefix("/openai/v1/")
    if re.fullmatch(r"/openai/deployments/[^/]+/chat/completions", path):
        return "/v1/chat/completions"
    # The AzureOpenAI client names the deployment in the body for the Responses API, not the path.
    if path == "/openai/responses":
        return "/v1/responses"
    return path


def _text(value: Any) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return "".join(
            item.get("text", "")
            for item in value
            if isinstance(item, dict) and "text" in item
        )
    return ""


# --- Chat Completions -------------------------------------------------------------------------


def chat_request(body: dict[str, Any]) -> script.Request:
    turns: list[script.Turn] = []
    names: dict[str, str] = {}
    for message in body.get("messages") or []:
        role = message.get("role")
        if role == "tool":
            call_id = message.get("tool_call_id", "")
            result = script.Result(
                call_id, names.get(call_id, ""), _text(message.get("content"))
            )
            if turns and turns[-1].role == "tool":
                turns[-1].results.append(result)
            else:
                turns.append(script.Turn(role="tool", results=[result]))
            continue
        turn = script.Turn(role="user" if role == "user" else role or "user")
        turn.text = _text(message.get("content"))
        for call in message.get("tool_calls") or []:
            function = call.get("function") or {}
            names[call.get("id", "")] = function.get("name", "")
            turn.calls.append(
                script.Call(
                    call.get("id", ""),
                    function.get("name", ""),
                    json.loads(function.get("arguments") or "{}"),
                )
            )
        if role in ("system", "developer"):
            continue
        turns.append(turn)
    tools = {
        tool["function"]["name"]: tool["function"].get("parameters") or {}
        for tool in body.get("tools") or []
        if tool.get("type") == "function"
    }
    response_format = body.get("response_format") or {}
    schema = (response_format.get("json_schema") or {}).get("schema")
    return script.Request(turns=turns, tools=tools, schema=schema)


def chat_message(answer: script.Reply) -> dict[str, Any]:
    message: dict[str, Any] = {"role": "assistant", "content": answer.text or None}
    if answer.calls:
        message["tool_calls"] = [
            {
                "id": call.id,
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": json.dumps(call.arguments),
                },
            }
            for call in answer.calls
        ]
    return message


def _usage() -> dict[str, int]:
    return {"prompt_tokens": 40, "completion_tokens": 12, "total_tokens": 52}


def completion(body: dict[str, Any]) -> dict[str, Any]:
    answer = script.reply(chat_request(body))
    return {
        "id": "chatcmpl-" + script.digest(body),
        "object": "chat.completion",
        "created": CREATED,
        "model": body.get("model", "fake"),
        "choices": [
            {
                "index": 0,
                "message": chat_message(answer),
                "finish_reason": "tool_calls" if answer.calls else "stop",
            }
        ],
        "usage": _usage(),
    }


def completion_chunks(body: dict[str, Any]) -> list[dict[str, Any]]:
    value = completion(body)
    choice = value["choices"][0]
    message = choice["message"]

    def event(delta: dict[str, Any], finish: str | None = None) -> dict[str, Any]:
        return {
            "id": value["id"],
            "object": "chat.completion.chunk",
            "created": CREATED,
            "model": value["model"],
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        }

    events = [event({"role": "assistant", "content": ""})]
    text = message.get("content") or ""
    if text:
        middle = len(text) // 2
        events += [event({"content": text[:middle]}), event({"content": text[middle:]})]
    for index, call in enumerate(message.get("tool_calls") or []):
        events.append(event({"tool_calls": [{"index": index, **call}]}))
    events.append(event({}, choice["finish_reason"]))
    if (body.get("stream_options") or {}).get("include_usage"):
        events.append({**event({}), "choices": [], "usage": _usage()})
    return events


# --- Responses --------------------------------------------------------------------------------


def responses_request(body: dict[str, Any]) -> script.Request:
    items = body.get("input")
    if isinstance(items, str):
        items = [{"role": "user", "content": items}]
    turns: list[script.Turn] = []
    names: dict[str, str] = {}
    for item in items or []:
        kind = item.get("type", "message")
        if kind == "function_call":
            names[item.get("call_id", "")] = item.get("name", "")
            call = script.Call(
                item.get("call_id", ""),
                item.get("name", ""),
                json.loads(item.get("arguments") or "{}"),
            )
            if turns and turns[-1].role == "assistant" and not turns[-1].results:
                turns[-1].calls.append(call)
            else:
                turns.append(script.Turn(role="assistant", calls=[call]))
        elif kind == "function_call_output":
            call_id = item.get("call_id", "")
            output = item.get("output")
            result = script.Result(
                call_id, names.get(call_id, ""), _text(output) or str(output)
            )
            if turns and turns[-1].role == "tool":
                turns[-1].results.append(result)
            else:
                turns.append(script.Turn(role="tool", results=[result]))
        elif kind == "message":
            role = item.get("role", "user")
            if role in ("system", "developer"):
                continue
            turns.append(script.Turn(role=role, text=_text(item.get("content"))))
    tools = {
        tool["name"]: tool.get("parameters") or {}
        for tool in body.get("tools") or []
        if tool.get("type") == "function"
    }
    text_format = (body.get("text") or {}).get("format") or {}
    schema = (
        text_format.get("schema") if text_format.get("type") == "json_schema" else None
    )
    reasoning = body.get("reasoning") or {}
    return script.Request(
        turns=turns, tools=tools, schema=schema, thinking=bool(reasoning.get("summary"))
    )


def response_output(answer: script.Reply, suffix: str) -> list[dict[str, Any]]:
    output: list[dict[str, Any]] = []
    if answer.thought:
        output.append(
            {
                "id": f"rs_{suffix}",
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": answer.thought}],
            }
        )
    if answer.text:
        output.append(
            {
                "id": f"msg_{suffix}",
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [
                    {"type": "output_text", "text": answer.text, "annotations": []}
                ],
            }
        )
    output.extend(
        {
            "id": f"fc_{call.id}",
            "type": "function_call",
            "call_id": call.id,
            "name": call.name,
            "arguments": json.dumps(call.arguments),
            "status": "completed",
        }
        for call in answer.calls
    )
    return output


def response(body: dict[str, Any]) -> dict[str, Any]:
    suffix = script.digest(body)
    answer = script.reply(responses_request(body))
    return {
        "id": f"resp_{suffix}",
        "object": "response",
        "created_at": CREATED,
        "completed_at": CREATED,
        "status": "completed",
        "model": body.get("model", "fake"),
        "instructions": body.get("instructions"),
        "output": response_output(answer, suffix),
        "parallel_tool_calls": body.get("parallel_tool_calls", True),
        "tool_choice": body.get("tool_choice", "auto"),
        "tools": body.get("tools") or [],
        "temperature": body.get("temperature"),
        "top_p": body.get("top_p"),
        "reasoning": body.get("reasoning"),
        "text": body.get("text") or {"format": {"type": "text"}},
        "metadata": body.get("metadata") or {},
        "error": None,
        "incomplete_details": None,
        "usage": {
            "input_tokens": 40,
            "input_tokens_details": {"cached_tokens": 0},
            "output_tokens": 12,
            "output_tokens_details": {"reasoning_tokens": 0},
            "total_tokens": 52,
        },
    }


def response_events(body: dict[str, Any]) -> list[dict[str, Any]]:
    final = response(body)
    started = {**final, "status": "in_progress", "output": [], "usage": None}
    events: list[dict[str, Any]] = [
        {"type": "response.created", "response": started},
        {"type": "response.in_progress", "response": started},
    ]
    for index, item in enumerate(final["output"]):
        events.append(
            {"type": "response.output_item.added", "output_index": index, "item": item}
        )
        if item["type"] == "message":
            text = item["content"][0]["text"]
            part = {"type": "output_text", "text": "", "annotations": []}
            events.append(
                {
                    "type": "response.content_part.added",
                    "item_id": item["id"],
                    "output_index": index,
                    "content_index": 0,
                    "part": part,
                }
            )
            middle = len(text) // 2
            for piece in (text[:middle], text[middle:]):
                events.append(
                    {
                        "type": "response.output_text.delta",
                        "item_id": item["id"],
                        "output_index": index,
                        "content_index": 0,
                        "delta": piece,
                        "logprobs": [],
                    }
                )
            events.append(
                {
                    "type": "response.output_text.done",
                    "item_id": item["id"],
                    "output_index": index,
                    "content_index": 0,
                    "text": text,
                    "logprobs": [],
                }
            )
            events.append(
                {
                    "type": "response.content_part.done",
                    "item_id": item["id"],
                    "output_index": index,
                    "content_index": 0,
                    "part": item["content"][0],
                }
            )
        elif item["type"] == "function_call":
            events.append(
                {
                    "type": "response.function_call_arguments.done",
                    "item_id": item["id"],
                    "output_index": index,
                    "arguments": item["arguments"],
                }
            )
        events.append(
            {"type": "response.output_item.done", "output_index": index, "item": item}
        )
    events.append({"type": "response.completed", "response": final})
    return [{**event, "sequence_number": number} for number, event in enumerate(events)]


# --- Server -----------------------------------------------------------------------------------


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format: str, *args: object) -> None:
        pass

    def send_json(self, status: int, value: dict[str, Any]) -> None:
        payload = json.dumps(value, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def send_events(self, events: list[dict[str, Any]], *, named: bool) -> None:
        lines = []
        for event in events:
            prefix = f"event: {event['type']}\n" if named else ""
            lines.append(
                f"{prefix}data: {json.dumps(event, separators=(',', ':'))}\n\n"
            )
        if not named:
            lines.append("data: [DONE]\n\n")
        payload = "".join(lines).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length) or b"{}")
        path = canonical_path(self.path)
        if path == "/v1/chat/completions":
            if body.get("stream"):
                self.send_events(completion_chunks(body), named=False)
            else:
                self.send_json(200, completion(body))
        elif path == "/v1/responses":
            if body.get("stream"):
                self.send_events(response_events(body), named=True)
            else:
                self.send_json(200, response(body))
        else:
            self.send_json(
                404, {"error": {"message": "unsupported endpoint", "type": "not_found"}}
            )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--port", type=int, default=PORT)
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"[fake-openai] listening on http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
