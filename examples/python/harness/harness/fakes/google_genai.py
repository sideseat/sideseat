"""A local Gemini endpoint, for the Gemini Developer API and Vertex AI routes of ``google-genai``.

Serves ``generateContent`` and ``streamGenerateContent`` (SSE) with the answers of
:mod:`harness.fakes.script`: function calls for the declared tools, thought parts when the request
asks for thoughts, and JSON when it sets a response schema.

    uv run --locked --directory examples/python/harness python -m harness.fakes.google_genai
"""

from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import urlsplit

from harness.fakes import script

PORT = 5404
MODEL_VERSION = "gemini-flash-latest"


def member(value: dict[str, Any], camel: str) -> Any:
    """A request member in either spelling: the API reads camelCase and snake_case alike, and the
    client sends some members in each."""
    snake = "".join(f"_{c.lower()}" if c.isupper() else c for c in camel)
    return value.get(camel, value.get(snake))


def request_of(body: dict[str, Any]) -> script.Request:
    turns = []
    for message in member(body, "contents") or []:
        role = "user" if message.get("role", "user") == "user" else "assistant"
        turn = script.Turn(role=role)
        for part in message.get("parts") or []:
            if part.get("thought"):
                continue
            if isinstance(part.get("text"), str):
                turn.text += part["text"]
            elif call := member(part, "functionCall"):
                turn.calls.append(
                    script.Call(
                        call.get("id", ""), call["name"], call.get("args") or {}
                    )
                )
            elif response := member(part, "functionResponse"):
                turn.results.append(
                    script.Result(
                        response.get("id", ""),
                        response["name"],
                        json.dumps(response.get("response")),
                    )
                )
        turns.append(turn)
    tools = {
        declaration["name"]: member(declaration, "parametersJsonSchema")
        or declaration.get("parameters")
        or {}
        for tool in member(body, "tools") or []
        for declaration in member(tool, "functionDeclarations") or []
    }
    config = member(body, "generationConfig") or {}
    schema = member(config, "responseJsonSchema") or member(config, "responseSchema")
    thinking = bool(member(member(config, "thinkingConfig") or {}, "includeThoughts"))
    return script.Request(turns=turns, tools=tools, schema=schema, thinking=thinking)


def parts_of(answer: script.Reply) -> list[dict[str, Any]]:
    parts: list[dict[str, Any]] = []
    if answer.thought:
        parts.append({"text": answer.thought, "thought": True})
    if answer.text:
        parts.append({"text": answer.text})
    parts.extend(
        {"functionCall": {"id": call.id, "name": call.name, "args": call.arguments}}
        for call in answer.calls
    )
    return parts


def chunk(
    parts: list[dict[str, Any]], *, final: bool, response_id: str
) -> dict[str, Any]:
    candidate: dict[str, Any] = {
        "content": {"role": "model", "parts": parts},
        "index": 0,
    }
    if final:
        candidate["finishReason"] = "STOP"
    output = sum(len(json.dumps(part)) // 4 for part in parts)
    return {
        "candidates": [candidate],
        "usageMetadata": {
            "promptTokenCount": 40,
            "candidatesTokenCount": output,
            "totalTokenCount": 40 + output,
        },
        "modelVersion": MODEL_VERSION,
        "responseId": response_id,
    }


def stream_of(parts: list[dict[str, Any]], response_id: str) -> list[dict[str, Any]]:
    """Text parts split in two, as a real stream delivers them; function calls arrive whole."""
    pieces: list[dict[str, Any]] = []
    for part in parts:
        text = part.get("text")
        if isinstance(text, str) and len(text) > 1:
            middle = len(text) // 2
            pieces += [{**part, "text": text[:middle]}, {**part, "text": text[middle:]}]
        else:
            pieces.append(part)
    return [
        chunk([piece], final=index == len(pieces) - 1, response_id=response_id)
        for index, piece in enumerate(pieces)
    ]


def is_model_path(path: str) -> bool:
    """Gemini Developer API (``/v1beta/models/...``) and Vertex AI publisher-model routes."""
    return "/models/" in path and (
        path.endswith(":generateContent") or path.endswith(":streamGenerateContent")
    )


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
        path = urlsplit(self.path).path
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length) or b"{}")
        if not is_model_path(path):
            self.send_json(
                404,
                {"error": {"message": "unsupported endpoint", "status": "NOT_FOUND"}},
            )
            return
        parts = parts_of(script.reply(request_of(body)))
        response_id = "gemini-" + script.digest(body)
        if path.endswith(":generateContent"):
            self.send_json(200, chunk(parts, final=True, response_id=response_id))
            return
        payload = "".join(
            f"data: {json.dumps(event, separators=(',', ':'))}\r\n\r\n"
            for event in stream_of(parts, response_id)
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
    print(f"[fake-gemini] listening on http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
