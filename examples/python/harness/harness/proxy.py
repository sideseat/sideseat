"""A recording proxy in front of Amazon Bedrock, so a scenario's model traffic can be replayed.

Two live runs of the same scenario get different answers from the model, which makes their telemetry
incomparable. The capture tool therefore records the model responses of the native run into a
cassette and replays them byte for byte to the SDK run: both runs then hold exactly the same
conversation, and any difference in their telemetry is the SDK's doing.

Committed cassettes also let a scenario be re-captured offline, without credentials, after a change to
the SDK or the server.

Clients reach the proxy through standard endpoint settings, set by :func:`client_environment`:
botocore's ``AWS_ENDPOINT_URL_BEDROCK_RUNTIME``, LiteLLM's ``AWS_BEDROCK_RUNTIME_ENDPOINT``, and
``SIDESEAT_MODEL_PROXY`` for the harness's OpenAI and Anthropic clients. In record mode the proxy
re-signs each request for the real endpoint with the ambient AWS credentials.
"""

from __future__ import annotations

import base64
import hashlib
import json
import threading
import urllib.error
import urllib.request
from collections import defaultdict, deque
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

from harness.models import region
from harness.scrub import scrub_body

# Headers that describe the payload rather than the transport; everything a client needs to parse
# the body. Content-Length is recomputed.
_KEPT_RESPONSE_HEADERS = ("content-type", "x-amzn-bedrock-content-type")


def client_environment(url: str) -> dict[str, str]:
    """Environment variables that send every supported model client through the proxy at ``url``."""
    return {
        "AWS_ENDPOINT_URL_BEDROCK_RUNTIME": url,
        "AWS_BEDROCK_RUNTIME_ENDPOINT": url,
        "SIDESEAT_MODEL_PROXY": url,
    }


class ModelProxy:
    def __init__(self, cassette: Path, *, record: bool) -> None:
        self.cassette = cassette
        self.record = record
        self.misses: list[str] = []
        #: How each replayed request found its answer: by an identical request, or - when the request
        #: changed, as a different framework version serialises it differently - by arrival order on
        #: the same method and path. Order is sound only while the conversation takes the same course,
        #: which :meth:`unanswered` lets a caller check.
        self.exact = 0
        self.by_order = 0
        self._recorded: list[dict[str, Any]] = []
        self._by_digest: dict[str, deque[dict[str, Any]]] = defaultdict(deque)
        self._by_path: dict[str, deque[dict[str, Any]]] = defaultdict(deque)
        self._lock = threading.Lock()
        if not record:
            for item in json.loads(cassette.read_text())["interactions"]:
                self._by_digest[item["request_sha256"]].append(item)
                self._by_path[f"{item['method']} {item['path']}"].append(item)
        self._server = ThreadingHTTPServer(("127.0.0.1", 0), self._handler())

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self._server.server_address[1]}"

    def __enter__(self) -> ModelProxy:
        threading.Thread(target=self._server.serve_forever, daemon=True).start()
        return self

    def __exit__(self, *exc: object) -> None:
        self._server.shutdown()
        if self.record and self._recorded:
            self.cassette.parent.mkdir(parents=True, exist_ok=True)
            self.cassette.write_text(
                json.dumps(
                    {"interactions": [_scrubbed(i) for i in self._recorded]}, indent=1
                )
                + "\n"
            )

    def _respond(
        self, method: str, path: str, body: bytes, headers: dict[str, str]
    ) -> dict[str, Any]:
        digest = hashlib.sha256(
            method.encode() + b" " + path.encode() + b"\n" + body
        ).hexdigest()
        if self.record:
            item = self._forward(method, path, body, headers)
            item["request_sha256"] = digest
            with self._lock:
                self._recorded.append(item)
            return item
        with self._lock:
            # An identical request gets its own recorded answer, which keeps concurrent requests
            # paired; a request that changed (a new framework version) falls back to arrival order.
            queue = self._by_digest.get(digest)
            item = queue.popleft() if queue else None
            if item is not None:
                self._by_path[f"{method} {path}"].remove(item)
                self.exact += 1
            else:
                fallback = self._by_path.get(f"{method} {path}")
                item = fallback.popleft() if fallback else None
                if item is not None:
                    self._by_digest[item["request_sha256"]].remove(item)
                    self.by_order += 1
        if item is None:
            self.misses.append(f"{method} {path}")
            return {
                "status": 599,
                "headers": {"content-type": "application/json"},
                "body": _b64(
                    json.dumps(
                        {"message": f"no recorded response for {method} {path}"}
                    ).encode()
                ),
            }
        return item

    def unanswered(self) -> list[str]:
        """Recorded interactions no request asked for: the replayed run took a shorter course."""
        with self._lock:
            return [key for key, queue in self._by_path.items() for _ in queue]

    @staticmethod
    def _forward(
        method: str, path: str, body: bytes, headers: dict[str, str]
    ) -> dict[str, Any]:
        import boto3
        from botocore.auth import SigV4Auth
        from botocore.awsrequest import AWSRequest

        url = f"https://bedrock-runtime.{region()}.amazonaws.com{path}"
        signed = AWSRequest(
            method=method,
            url=url,
            data=body,
            headers={
                "Content-Type": headers.get("content-type", "application/json"),
                "Accept": headers.get("accept", "application/json"),
            },
        )
        credentials = boto3.Session().get_credentials()
        if credentials is None:
            raise RuntimeError("recording needs AWS credentials")
        SigV4Auth(credentials.get_frozen_credentials(), "bedrock", region()).add_auth(
            signed
        )
        request = urllib.request.Request(
            url, data=body or None, method=method, headers=dict(signed.headers)
        )
        try:
            with urllib.request.urlopen(request, timeout=300) as response:
                status, raw, response_headers = (
                    response.status,
                    response.read(),
                    response.headers,
                )
        except urllib.error.HTTPError as error:
            status, raw, response_headers = error.code, error.read(), error.headers
        kept = {k: v for k in _KEPT_RESPONSE_HEADERS if (v := response_headers.get(k))}
        return {
            "method": method,
            "path": path,
            "status": status,
            "headers": kept,
            "body": _b64(raw),
        }

    def _handler(self) -> type[BaseHTTPRequestHandler]:
        proxy = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, fmt: str, *args: object) -> None:
                pass

            def _serve(self) -> None:
                length = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(length) if length else b""
                headers = {k.lower(): v for k, v in self.headers.items()}
                item = proxy._respond(self.command, self.path, body, headers)
                payload = base64.b64decode(item["body"])
                self.send_response(item["status"])
                for name, value in item["headers"].items():
                    self.send_header(name, value)
                self.send_header("Content-Length", str(len(payload)))
                # The handler speaks HTTP/1.0 and closes every connection; a client that pools
                # connections (OkHttp does) would otherwise send its next request into the closed one.
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(payload)

            do_POST = _serve  # noqa: N815 - BaseHTTPRequestHandler API
            do_GET = _serve  # noqa: N815

        return Handler


def _scrubbed(item: dict[str, Any]) -> dict[str, Any]:
    """A recorded interaction with the capturing account's name removed from its decoded body.

    Scrubbed only as the cassette is written: the live run must see what the model said, or a tool
    whose name the model echoes would no longer match. A replay then answers with the placeholder.
    """
    body = base64.b64decode(item["body"])
    clean = scrub_body(body, item.get("headers", {}).get("content-type", ""))
    return item if clean == body else {**item, "body": _b64(clean)}


def _b64(raw: bytes) -> str:
    return base64.b64encode(raw).decode("ascii")
