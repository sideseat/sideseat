"""The model requests one fixture's run actually sent: ``<fixture>/model-requests.json``.

A cassette keeps only the digest of each request, and one cassette serves the native and SDK modes and
every release of a framework, which do not send the same bytes. The request truth (rubric v3) needs
what *this* fixture's framework sent, so every capture path records it: the recording proxy, whether
it records or replays a cassette, and the fake model servers. Each request is kept as received, scrubbed
of the capturing account like a cassette body.

While a run is in progress, requests are appended as JSON lines to the file the environment variable
:data:`ENV` names, from whichever process serves the model; :func:`finish` turns that log into the
committed document.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import threading
from pathlib import Path
from typing import Any, Callable

from harness.scrub import scrub_body

FORMAT = "sideseat.transcript/1"
FILENAME = "model-requests.json"
#: Where the process serving a model appends each request it receives.
ENV = "SIDESEAT_MODEL_TRANSCRIPT"

_lock = threading.Lock()


def digest(method: str, path: str, body: bytes) -> str:
    """The request digest a cassette pairs answers by - of the unscrubbed bytes."""
    return hashlib.sha256(
        method.encode() + b" " + path.encode() + b"\n" + body
    ).hexdigest()


def record(
    method: str,
    path: str,
    body: bytes,
    content_type: str,
    *,
    answered_by: int | None = None,
    log: str | None = None,
) -> None:
    """Append one received request to the run's log; nothing when no log is configured."""
    target = log or os.environ.get(ENV)
    if not target:
        return
    entry: dict[str, Any] = {
        "method": method,
        "path": path,
        "content_type": content_type,
        "request_sha256": digest(method, path, body),
        "body": base64.b64encode(body).decode("ascii"),
    }
    if answered_by is not None:
        entry["answered_by"] = answered_by
    line = json.dumps(entry, sort_keys=True) + "\n"
    with _lock, open(target, "a", encoding="utf-8") as out:
        out.write(line)


def finish(
    log: Path, anonymise: Callable[[bytes], bytes] | None = None
) -> dict[str, Any]:
    """The committed document for a run's log: requests in arrival order, bodies scrubbed.

    ``anonymise`` is the run's own telemetry anonymisation, with the names it pinned: what a request
    carries must read exactly as the fixture's telemetry does, or the request truth would demand a
    session directory or a tab name the fixture never shows.
    """
    interactions = []
    if log.exists():
        for line in log.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            entry = json.loads(line)
            raw = base64.b64decode(entry["body"])
            if anonymise is not None:
                raw = anonymise(raw)
            clean = scrub_body(raw, entry.get("content_type", ""))
            entry["body"] = base64.b64encode(clean).decode("ascii")
            interactions.append(entry)
    return {"format": FORMAT, "interactions": interactions}


def load(path: Path) -> list[dict[str, Any]]:
    """A committed transcript's requests, each with its decoded ``body`` bytes."""
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("format") != FORMAT:
        raise ValueError(f"{path}: not a {FORMAT} document")
    return [
        {**entry, "body": base64.b64decode(entry["body"])}
        for entry in document["interactions"]
    ]
