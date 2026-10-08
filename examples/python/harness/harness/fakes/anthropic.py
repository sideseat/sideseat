"""A local Anthropic Messages endpoint, streamed or not, answering from :mod:`harness.fakes.script`.

uv run --locked --directory examples/python/harness python -m harness.fakes.anthropic
"""

from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import urlsplit

from harness.fakes import script

PORT = 5402

#: The tool families Anthropic runs on its own servers, by the prefix of their dated type names.
SERVER_TOOLS = (
    "web_search",
    "web_fetch",
    "code_execution",
    "tool_search_tool",
    "advisor",
)


def _text(value: Any) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return "".join(
            block.get("text", "")
            for block in value
            if isinstance(block, dict) and block.get("type") == "text"
        )
    return ""


def server_tool(tool: dict[str, Any]) -> str | None:
    """The server tool family a declared tool belongs to, or ``None`` for one the client runs.

    Dated releases (`web_search_20250305`, `web_search_20260318`) are one tool. Every other tool runs on
    the client, typed or not: the Anthropic-defined `bash`, `text_editor` and `memory` tools are the
    application's to execute.
    """
    kind = str(tool.get("type") or "")
    return next((f for f in SERVER_TOOLS if kind.startswith(f + "_")), None)


def request_of(body: dict[str, Any]) -> script.Request:
    turns: list[script.Turn] = []
    names: dict[str, str] = {}
    for message in body.get("messages") or []:
        turn = script.Turn(role=message.get("role", "user"))
        blocks = message.get("content")
        turn.text = _text(blocks)
        for block in blocks if isinstance(blocks, list) else []:
            if block.get("type") == "tool_use":
                names[block["id"]] = block["name"]
                turn.calls.append(
                    script.Call(block["id"], block["name"], block.get("input") or {})
                )
            elif block.get("type") == "tool_result":
                turn.results.append(
                    script.Result(
                        block["tool_use_id"],
                        names.get(block["tool_use_id"], ""),
                        _text(block.get("content")),
                    )
                )
        turns.append(turn)
    declared = body.get("tools") or []
    hosted = {family for tool in declared if (family := server_tool(tool))}
    # A toolset (`mcp_toolset`, `computer_toolset_20260801`) names no tool of its own: the tools it
    # stands for are configured elsewhere, and the script has none to call.
    tools = {
        tool["name"]: tool.get("input_schema") or {}
        for tool in declared
        if server_tool(tool) is None and "name" in tool
    }
    output_format = (body.get("output_config") or {}).get("format") or {}
    schema = (
        output_format.get("schema")
        if output_format.get("type") == "json_schema"
        else None
    )
    thinking = (body.get("thinking") or {}).get("type") in ("adaptive", "enabled")
    return script.Request(
        turns=turns,
        tools=tools,
        schema=schema,
        thinking=thinking,
        hosted_tools=hosted,
    )


def search_blocks(search: script.ServerCall) -> list[dict[str, Any]]:
    """A search the provider ran: its ``server_tool_use`` and the ``web_search_tool_result`` it found.

    The search is a direct call, as ``web_search_20250305`` makes; a later release that filters
    results in code execution nests the pair under a ``caller``, which the fake does not model.
    """
    call_id = "srvtoolu_" + search.id.removeprefix("ws_")
    return [
        {
            "type": "server_tool_use",
            "id": call_id,
            "name": search.tool,
            "input": {"query": search.query},
        },
        {
            "type": "web_search_tool_result",
            "tool_use_id": call_id,
            "content": [
                {
                    "type": "web_search_result",
                    "url": url,
                    "title": search.query,
                    "encrypted_content": "enc_" + script.digest(url),
                }
                for url in search.sources
            ],
        },
    ]


def citations(answer: script.Reply) -> list[dict[str, Any]]:
    """Where an answer drawn from a search read what it says: one location per source it found."""
    return [
        {
            "type": "web_search_result_location",
            "url": url,
            "title": search.query,
            "encrypted_index": "idx_" + script.digest(url),
            "cited_text": answer.text[:150],
        }
        for search in answer.server_calls
        for url in search.sources
    ]


def message(body: dict[str, Any]) -> dict[str, Any]:
    answer = script.reply(request_of(body))
    blocks: list[dict[str, Any]] = []
    if answer.thought:
        blocks.append(
            {"type": "thinking", "thinking": answer.thought, "signature": "fake"}
        )
    for search in answer.server_calls:
        blocks.extend(search_blocks(search))
    if answer.text:
        text: dict[str, Any] = {"type": "text", "text": answer.text}
        if answer.server_calls:
            text["citations"] = citations(answer)
        blocks.append(text)
    blocks.extend(
        {"type": "tool_use", "id": call.id, "name": call.name, "input": call.arguments}
        for call in answer.calls
    )
    usage: dict[str, Any] = {"input_tokens": 40, "output_tokens": 12}
    if answer.server_calls:
        usage["server_tool_use"] = {
            "web_search_requests": len(answer.server_calls),
            "web_fetch_requests": 0,
        }
    return {
        "id": "msg_" + script.digest(body),
        "type": "message",
        "role": "assistant",
        "model": body.get("model", "fake"),
        "content": blocks,
        "stop_reason": "tool_use" if answer.calls else "end_turn",
        "stop_sequence": None,
        "usage": usage,
    }


def events_of(value: dict[str, Any]) -> list[tuple[str, dict[str, Any]]]:
    start = {
        **value,
        "content": [],
        "stop_reason": None,
        "usage": {"input_tokens": 40, "output_tokens": 0},
    }
    events: list[tuple[str, dict[str, Any]]] = [
        ("message_start", {"type": "message_start", "message": start})
    ]
    for index, block in enumerate(value["content"]):
        if block["type"] == "text":
            opening = {"type": "text", "text": ""}
            deltas = [{"type": "text_delta", "text": block["text"]}]
            # Each citation streams on its own after the text it supports.
            deltas += [
                {"type": "citations_delta", "citation": citation}
                for citation in block.get("citations", [])
            ]
        elif block["type"] == "thinking":
            opening = {"type": "thinking", "thinking": "", "signature": ""}
            deltas = [
                {"type": "thinking_delta", "thinking": block["thinking"]},
                {"type": "signature_delta", "signature": block["signature"]},
            ]
        elif block["type"] == "web_search_tool_result":
            # A search's results arrive whole when its block opens.
            opening, deltas = block, []
        else:
            opening = {**block, "input": {}}
            deltas = [
                {"type": "input_json_delta", "partial_json": json.dumps(block["input"])}
            ]
        events.append(
            (
                "content_block_start",
                {
                    "type": "content_block_start",
                    "index": index,
                    "content_block": opening,
                },
            )
        )
        events += [
            (
                "content_block_delta",
                {"type": "content_block_delta", "index": index, "delta": delta},
            )
            for delta in deltas
        ]
        events.append(
            ("content_block_stop", {"type": "content_block_stop", "index": index})
        )
    events.append(
        (
            "message_delta",
            {
                "type": "message_delta",
                "delta": {"stop_reason": value["stop_reason"], "stop_sequence": None},
                # The final usage, without what message_start already reported.
                "usage": {
                    key: count
                    for key, count in value["usage"].items()
                    if key != "input_tokens"
                },
            },
        )
    )
    events.append(("message_stop", {"type": "message_stop"}))
    return events


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

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length) or b"{}")
        if urlsplit(self.path).path.rstrip("/") != "/v1/messages":
            self.send_json(
                404,
                {
                    "type": "error",
                    "error": {"type": "not_found_error", "message": "not found"},
                },
            )
            return
        value = message(body)
        if not body.get("stream"):
            self.send_json(200, value)
            return
        payload = "".join(
            f"event: {name}\ndata: {json.dumps(event, separators=(',', ':'))}\n\n"
            for name, event in events_of(value)
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--port", type=int, default=PORT)
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"[fake-anthropic] listening on http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
