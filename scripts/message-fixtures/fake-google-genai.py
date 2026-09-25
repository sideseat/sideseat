#!/usr/bin/env python3
"""Deterministic local Gemini endpoint for Google GenAI fixture capture."""

from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import urlsplit

MODEL = "gemini-2.5-flash"


def usage(output_tokens: int = 8) -> dict[str, int]:
    """Return stable Gemini usage metadata."""
    return {
        "promptTokenCount": 12,
        "candidatesTokenCount": output_tokens,
        "totalTokenCount": 12 + output_tokens,
    }


def parts(contents: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Flatten content parts from a GenerateContent request."""
    return [
        part
        for content in contents
        for part in content.get("parts", [])
        if isinstance(part, dict)
    ]


def text_response(text: str, *, response_id: str = "gemini-local") -> dict[str, Any]:
    """Build one non-streaming Gemini response."""
    return {
        "candidates": [
            {
                "content": {
                    "parts": [{"text": text}],
                    "role": "model",
                },
                "finishReason": "STOP",
                "index": 0,
            }
        ],
        "usageMetadata": usage(),
        "modelVersion": MODEL,
        "responseId": response_id,
    }


def response(body: dict[str, Any]) -> dict[str, Any]:
    """Return the deterministic response for one request body."""
    contents = body.get("contents", [])
    request_parts = parts(contents)
    has_tool_result = any("functionResponse" in part for part in request_parts)

    if body.get("tools") and not has_tool_result:
        return {
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "functionCall": {
                                    "id": "weather-call-1",
                                    "name": "get_weather",
                                    "args": {"location": "Paris"},
                                }
                            }
                        ],
                        "role": "model",
                    },
                    "finishReason": "STOP",
                    "index": 0,
                }
            ],
            "usageMetadata": usage(5),
            "modelVersion": MODEL,
            "responseId": "gemini-tool-call",
        }

    last_text = next(
        (
            part["text"]
            for part in reversed(request_parts)
            if isinstance(part.get("text"), str)
        ),
        "",
    )
    if has_tool_result:
        text = "It is sunny and 22°C in Paris."
    elif "boiling point" in last_text.lower():
        text = "Water boils at 100°C at sea level."
    elif "speed of light" in last_text.lower():
        text = "The speed of light is 299,792,458 metres per second."
    else:
        text = "This is a deterministic local response."
    return text_response(text)


class Handler(BaseHTTPRequestHandler):
    """Serve the small subset of the Gemini API exercised by the fixture."""

    protocol_version = "HTTP/1.1"

    def log_message(self, format: str, *args: object) -> None:
        print(f"[fake-google-genai] {format % args}", flush=True)

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
        self.send_json(404, {"error": {"message": "not found", "status": "NOT_FOUND"}})

    def do_POST(self) -> None:
        path = urlsplit(self.path).path
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length) or b"{}")

        if not path.startswith("/v1beta/models/"):
            self.send_json(
                404,
                {"error": {"message": "unsupported endpoint", "status": "NOT_FOUND"}},
            )
            return

        value = response(body)
        if path.endswith(":generateContent"):
            self.send_json(200, value)
            return
        if not path.endswith(":streamGenerateContent"):
            self.send_json(
                404,
                {"error": {"message": "unsupported operation", "status": "NOT_FOUND"}},
            )
            return

        text = value["candidates"][0]["content"]["parts"][0]["text"]
        midpoint = max(1, len(text) // 2)
        chunks = [
            {
                "candidates": [
                    {
                        "content": {
                            "parts": [{"text": text[:midpoint]}],
                            "role": "model",
                        },
                        "index": 0,
                    }
                ],
                "usageMetadata": usage(0),
                "modelVersion": MODEL,
                "responseId": "gemini-stream",
            },
            {
                "candidates": [
                    {
                        "content": {
                            "parts": [{"text": text[midpoint:]}],
                            "role": "model",
                        },
                        "finishReason": "STOP",
                        "index": 0,
                    }
                ],
                "usageMetadata": usage(),
                "modelVersion": MODEL,
                "responseId": "gemini-stream",
            },
        ]
        payload = "".join(
            f"data: {json.dumps(chunk, separators=(',', ':'))}\n\n" for chunk in chunks
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
        self.wfile.flush()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=5404)
    args = parser.parse_args()

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"[fake-google-genai] listening on http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
