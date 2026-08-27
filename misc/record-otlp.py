#!/usr/bin/env python3
"""Record OTLP trace payloads from a sample run, then forward them to SideSeat.

Sits between a sample and the server so the exact bytes a framework emits are captured as
test fixtures. Those fixtures drive server/src/domain/traces/message_goldens_tests.rs, which
replays them through the real extraction + SideML feed pipeline and compares the resulting
messages against committed expectations.

Recording the wire payload rather than the database rows matters: it is the only input the
server actually receives, so a fixture cannot drift from what the framework really sends.

Usage:
    # terminal 1
    python3 misc/record-otlp.py --label strands/tool_use

    # terminal 2 - point the sample at the recorder instead of SideSeat
    OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:5399/otel/default \\
      uv run --directory misc/samples/python/strands strands tool_use

Each POST body is written to
    server/tests/fixtures/messages/<label>/req-NNN.pb
and forwarded unchanged to --upstream (set --no-forward to only record).
"""

from __future__ import annotations

import argparse
import gzip
import sys
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
FIXTURE_ROOT = REPO_ROOT / "server" / "tests" / "fixtures" / "messages"


class Recorder(BaseHTTPRequestHandler):
    label: str
    upstream: str
    forward: bool
    counter = 0

    def log_message(self, fmt: str, *args: object) -> None:
        # Quieter than the default: one line per captured request, printed below.
        pass

    def _read_body(self) -> bytes:
        """Read the request body, honouring chunked transfer encoding.

        Reading only Content-Length silently produced an EMPTY body for every client that
        streams its export - the OpenTelemetry JS exporter and the Claude Code CLI both do -
        and the empty body was then forwarded to SideSeat, which answered 400 "Failed to decode
        JSON request". That looked exactly like a server defect until the path log showed
        `0 bytes`.
        """
        if (self.headers.get("Transfer-Encoding") or "").lower() == "chunked":
            chunks = []
            while True:
                line = self.rfile.readline().strip()
                if not line:
                    break
                try:
                    size = int(line.split(b";")[0], 16)
                except ValueError:
                    break
                if size == 0:
                    self.rfile.readline()  # trailing CRLF after the final chunk
                    break
                chunks.append(self.rfile.read(size))
                self.rfile.readline()  # CRLF after each chunk
            return b"".join(chunks)

        length = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(length) if length else b""

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        body = self._read_body()

        # Log every POST path: a client whose endpoint differs even slightly (a query string,
        # a different suffix) would otherwise be silently forwarded and never recorded, which
        # looks identical to "the sample produced no telemetry".
        print(f"[record] POST {self.path} ({len(body)} bytes, {self.headers.get('Content-Type')})", flush=True)

        # Only trace payloads are fixture material; metrics/logs are forwarded untouched.
        # Match on a path *containing* /v1/traces rather than ending with it: a query string
        # makes endswith() miss, and the request is still a trace export.
        if "/v1/traces" in self.path and body:
            Recorder.counter += 1
            out_dir = FIXTURE_ROOT / self.label
            out_dir.mkdir(parents=True, exist_ok=True)
            raw = body
            if self.headers.get("Content-Encoding") == "gzip":
                # Store decompressed so the fixture is decodable without knowing the encoding.
                try:
                    raw = gzip.decompress(body)
                except OSError:
                    pass
            suffix = "json" if self.headers.get("Content-Type", "").startswith("application/json") else "pb"
            path = out_dir / f"req-{Recorder.counter:03d}.{suffix}"
            path.write_bytes(raw)
            print(f"[record] {path.relative_to(REPO_ROOT)} ({len(raw)} bytes)", flush=True)

        status, resp_body = 200, b"{}"
        if self.forward:
            status, resp_body = self._forward(body)

        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(resp_body)))
        self.end_headers()
        self.wfile.write(resp_body)

    def _forward(self, body: bytes) -> tuple[int, bytes]:
        url = self.upstream.rstrip("/") + self.path
        headers = {
            k: v
            for k, v in self.headers.items()
            # transfer-encoding is dropped too: the body has been fully read, so urllib sets
            # a Content-Length and a stale `chunked` header would make the upstream misparse.
            if k.lower() not in ("host", "content-length", "connection", "transfer-encoding")
        }
        req = urllib.request.Request(url, data=body, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return r.status, r.read()
        except urllib.error.HTTPError as e:
            return e.code, e.read()
        except urllib.error.URLError as e:
            print(f"[record] upstream unreachable: {e}", file=sys.stderr, flush=True)
            return 200, b"{}"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--label", required=True, help="fixture path, e.g. strands/tool_use")
    ap.add_argument("--port", type=int, default=5399)
    ap.add_argument("--upstream", default="http://127.0.0.1:5388")
    ap.add_argument("--no-forward", action="store_true", help="record only, do not forward")
    args = ap.parse_args()

    Recorder.label = args.label
    Recorder.upstream = args.upstream
    Recorder.forward = not args.no_forward
    Recorder.counter = 0

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Recorder)
    dest = FIXTURE_ROOT / args.label

    # Numbering restarts at req-001 every run, so anything left from a longer previous capture
    # would survive with a higher number and be replayed as part of this one. Clear first.
    if dest.exists():
        stale = sorted(p for p in dest.glob("req-*") if p.is_file())
        for p_ in stale:
            p_.unlink()
        if stale:
            print(f"[record] cleared {len(stale)} stale payload(s) in {dest.relative_to(REPO_ROOT)}", flush=True)
    print(f"[record] listening on http://127.0.0.1:{args.port}", flush=True)
    print(f"[record] writing to {dest.relative_to(REPO_ROOT)}", flush=True)
    print(f"[record] forwarding to {args.upstream if Recorder.forward else '(disabled)'}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print(f"\n[record] captured {Recorder.counter} trace request(s)", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
