#!/usr/bin/env python3
"""Deterministic local OpenAI-compatible endpoint for framework fixture capture."""

from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


def usage() -> dict[str, int]:
    return {
        "prompt_tokens": 12,
        "completion_tokens": 7,
        "total_tokens": 19,
    }


def completion(body: dict[str, Any]) -> dict[str, Any]:
    messages = body.get("messages", [])
    model = body.get("model", "sideseat-local")
    has_tool_result = any(message.get("role") == "tool" for message in messages)

    if body.get("tools") and not has_tool_result:
        message: dict[str, Any] = {
            "role": "assistant",
            "content": None,
            "tool_calls": [
                {
                    "id": "call-weather-1",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": '{"location":"Paris"}',
                    },
                }
            ],
        }
        finish_reason = "tool_calls"
    else:
        last_user = next(
            (
                message.get("content", "")
                for message in reversed(messages)
                if message.get("role") == "user"
            ),
            "",
        )
        if has_tool_result:
            text = "It is sunny and 22°C in Paris."
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

    def do_GET(self) -> None:
        if self.path == "/health":
            self.send_json(200, {"status": "ok"})
            return
        self.send_json(404, {"error": {"message": "not found", "type": "not_found"}})

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length) or b"{}")
        if self.path.rstrip("/") != "/v1/chat/completions":
            self.send_json(
                404,
                {"error": {"message": "unsupported endpoint", "type": "not_found"}},
            )
            return
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

        value = completion(body)
        if not body.get("stream"):
            self.send_json(200, value)
            return

        text = value["choices"][0]["message"]["content"]
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
            {
                "id": value["id"],
                "object": "chat.completion.chunk",
                "created": value["created"],
                "model": value["model"],
                "choices": [
                    {
                        "index": 0,
                        "delta": {"content": text},
                        "finish_reason": None,
                    }
                ],
            },
            {
                "id": value["id"],
                "object": "chat.completion.chunk",
                "created": value["created"],
                "model": value["model"],
                "choices": [
                    {
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop",
                    }
                ],
                "usage": usage(),
            },
        ]
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
