#!/usr/bin/env python3
"""Deterministic local Anthropic Messages endpoint for framework fixture capture."""

from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


def usage(output_tokens: int = 7) -> dict[str, int]:
    return {
        "input_tokens": 12,
        "output_tokens": output_tokens,
    }


def has_tool_result(messages: list[dict[str, Any]]) -> bool:
    return any(
        isinstance(message.get("content"), list)
        and any(
            isinstance(block, dict) and block.get("type") == "tool_result"
            for block in message["content"]
        )
        for message in messages
    )


def message(body: dict[str, Any]) -> dict[str, Any]:
    messages = body.get("messages", [])
    model = body.get("model", "sideseat-local")

    if body.get("tools") and not has_tool_result(messages):
        content: list[dict[str, Any]] = [
            {
                "type": "tool_use",
                "id": "toolu_weather_1",
                "name": "get_weather",
                "input": {"location": "Paris"},
            }
        ]
        stop_reason = "tool_use"
    else:
        last_user = next(
            (
                entry.get("content", "")
                for entry in reversed(messages)
                if entry.get("role") == "user"
            ),
            "",
        )
        if has_tool_result(messages):
            text = "It is sunny and 22°C in Paris."
        elif "boiling point" in str(last_user).lower():
            text = "Water boils at 100°C at sea level."
        elif "speed of light" in str(last_user).lower():
            text = "The speed of light is 299,792,458 metres per second."
        else:
            text = "This is a deterministic local response."
        content = [{"type": "text", "text": text}]
        stop_reason = "end_turn"

    return {
        "id": "msg_sideseat_local",
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": None,
        "usage": usage(),
    }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format: str, *args: object) -> None:
        print(f"[fake-anthropic] {format % args}", flush=True)

    def send_json(self, status: int, value: dict[str, Any]) -> None:
        payload = json.dumps(value, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("request-id", "req_sideseat_local")
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self) -> None:
        if self.path == "/health":
            self.send_json(200, {"status": "ok"})
            return
        self.send_json(
            404,
            {
                "type": "error",
                "error": {"type": "not_found_error", "message": "not found"},
            },
        )

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length) or b"{}")
        if self.path.rstrip("/") != "/v1/messages":
            self.send_json(
                404,
                {
                    "type": "error",
                    "error": {
                        "type": "not_found_error",
                        "message": "unsupported endpoint",
                    },
                },
            )
            return
        if body.get("model") == "nonexistent-model-id-12345":
            self.send_json(
                404,
                {
                    "type": "error",
                    "error": {
                        "type": "not_found_error",
                        "message": "model not found",
                    },
                },
            )
            return

        value = message(body)
        if not body.get("stream"):
            self.send_json(200, value)
            return

        text = value["content"][0]["text"]
        events = [
            (
                "message_start",
                {
                    "type": "message_start",
                    "message": {
                        **value,
                        "content": [],
                        "stop_reason": None,
                        "usage": usage(0),
                    },
                },
            ),
            (
                "content_block_start",
                {
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": ""},
                },
            ),
            (
                "content_block_delta",
                {
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": text},
                },
            ),
            (
                "content_block_stop",
                {"type": "content_block_stop", "index": 0},
            ),
            (
                "message_delta",
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                    "usage": {"output_tokens": 7},
                },
            ),
            ("message_stop", {"type": "message_stop"}),
        ]
        payload = "".join(
            f"event: {name}\ndata: {json.dumps(event, separators=(',', ':'))}\n\n"
            for name, event in events
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("request-id", "req_sideseat_local")
        self.end_headers()
        self.wfile.write(payload)
        self.wfile.flush()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=5402)
    args = parser.parse_args()

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"[fake-anthropic] listening on http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
