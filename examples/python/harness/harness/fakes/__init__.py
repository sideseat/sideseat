"""Local fake model servers, for scenarios that run without credentials.

Each module serves one provider's wire format from the deterministic script in
:mod:`harness.fakes.script`. :func:`start` runs one inside the current process, which is how the
``fake-*`` model aliases reach a server without one being started by hand.
"""

from __future__ import annotations

import importlib
import io
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from harness import transcript

#: The fake module serving each ``fake-*`` model surface.
MODULES = {
    "fake-openai": "harness.fakes.openai",
    "fake-anthropic": "harness.fakes.anthropic",
    "fake-gemini": "harness.fakes.google_genai",
}


def recording(handler: type[BaseHTTPRequestHandler]) -> type[BaseHTTPRequestHandler]:
    """``handler`` with every POST it answers appended to the run's request transcript."""

    class Recording(handler):  # type: ignore[valid-type, misc]
        def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
            length = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(length) if length else b""
            transcript.record(
                "POST", self.path, body, self.headers.get("Content-Type", "")
            )
            self.rfile = io.BytesIO(body)
            super().do_POST()

    return Recording


def start(surface: str) -> str:
    """Serves ``surface`` from a daemon thread on a free port; returns its base URL."""
    module = importlib.import_module(MODULES[surface])
    server = ThreadingHTTPServer(("127.0.0.1", 0), recording(module.Handler))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{server.server_address[1]}"
