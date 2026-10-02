"""Local fake model servers, for scenarios that run without credentials.

Each module serves one provider's wire format from the deterministic script in
:mod:`harness.fakes.script`. :func:`start` runs one inside the current process, which is how the
``fake-*`` model aliases reach a server without one being started by hand.
"""

from __future__ import annotations

import importlib
import threading
from http.server import ThreadingHTTPServer

#: The fake module serving each ``fake-*`` model surface.
MODULES = {
    "fake-openai": "harness.fakes.openai",
    "fake-anthropic": "harness.fakes.anthropic",
    "fake-gemini": "harness.fakes.google_genai",
}


def start(surface: str) -> str:
    """Serves ``surface`` from a daemon thread on a free port; returns its base URL."""
    module = importlib.import_module(MODULES[surface])
    server = ThreadingHTTPServer(("127.0.0.1", 0), module.Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{server.server_address[1]}"
