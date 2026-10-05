"""A static travel site served on localhost, so the browser never reaches the public internet.

The pages are generated from the shared tools in :mod:`harness.content`. The port is fixed because
page URLs reach the model's requests, which the native and SDK runs of a scenario must share.
"""

from __future__ import annotations

import html
import os
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from harness import content

PORT = int(os.getenv("BROWSER_USE_SITE_PORT") or 47811)
URL = f"http://127.0.0.1:{PORT}"
CITIES = ("Paris", "Tokyo", "Rome", "Oslo")


def _page(title: str, body: str) -> bytes:
    return (
        "<!doctype html><html lang='en'><head><meta charset='utf-8'>"
        f"<title>{html.escape(title)}</title></head>"
        f"<body><h1>{html.escape(title)}</h1>{body}</body></html>"
    ).encode()


def _home() -> bytes:
    return _page(
        "SideSeat Travel",
        "<p>Plan your next trip.</p>"
        "<ul><li><a href='/weather'>Weather forecasts</a></li></ul>",
    )


def _weather() -> bytes:
    rows = "".join(
        f"<tr><td>{city}</td><td>Day {day['day']}</td><td>{day['condition']}</td>"
        f"<td>{day['high_c']} °C</td></tr>"
        for city in CITIES
        for day in content.get_weather(city, 2)["forecast"]  # type: ignore[union-attr]
    )
    return _page(
        "Two-day forecast",
        "<table><thead><tr><th>City</th><th>Day</th><th>Condition</th><th>High</th></tr></thead>"
        f"<tbody>{rows}</tbody></table><p><a href='/'>Home</a></p>",
    )


PAGES = {"/": _home, "/weather": _weather}


class _Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt: str, *args: object) -> None:
        pass

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        render = PAGES.get(self.path.split("?")[0])
        body = render() if render else _page("Not found", "<p>No such page.</p>")
        self.send_response(200 if render else 404)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


@contextmanager
def serve() -> Iterator[str]:
    """Serve the site for the duration of a scenario and yield its base URL."""
    try:
        server = ThreadingHTTPServer(("127.0.0.1", PORT), _Handler)
    except OSError as error:
        raise SystemExit(
            f"port {PORT} is taken; set BROWSER_USE_SITE_PORT to a free one for both modes"
        ) from error
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield URL
    finally:
        server.shutdown()
        server.server_close()
