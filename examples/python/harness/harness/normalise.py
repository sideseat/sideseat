"""``python -m harness normalise``: the capture's path normalisation applied to committed recordings.

Every recording a capture writes passes through ``harness.paths``; this applies the same functions to what is
already committed, so a fixture recorded before the normalisation existed reads as a fresh capture would write it:
OTLP exports (``req-*``, ``logs-*``, ``metrics-*``), transcripts (``model-requests.json``, each body and its
digest), cassettes (each response body) and the replay archives
(``*.jsonl.gz``, streamed line by line and recompressed with no timestamp or name). ``--check`` changes nothing
and fails when a file would change. Running it twice changes nothing the second time.

A cassette keeps no request body, only the digest of one, so its digests stay as recorded: the proxy pairs a
request by its normalised digest and by the raw one a cassette recorded before digests were normalised, so no
recording loses the exact pairing it had.
"""

from __future__ import annotations

import base64
import gzip
import json
import os
import sys
import tempfile
import zlib
from collections.abc import Callable, Iterator
from pathlib import Path

from harness import transcript
from harness.paths import normalise_body, normalise_otlp, normalise_response

REPO = Path(__file__).resolve().parents[4]
FIXTURES = REPO / "server/tests/fixtures"
CASSETTES = REPO / "examples"
ARCHIVES = REPO / "scripts/tools/otel-replay/fixtures"


def _document(document: dict) -> bytes:
    return (json.dumps(document, indent=1) + "\n").encode("utf-8")


def _export(path: Path) -> bytes:
    signal = path.name.split("-", 1)[0]
    return normalise_otlp(path.read_bytes(), signal, path.suffix.lstrip("."))


def _transcript(path: Path) -> bytes:
    raw = path.read_bytes()
    document = json.loads(raw)
    for entry in document["interactions"]:
        body = base64.b64decode(entry["body"])
        clean = normalise_body(body, entry.get("content_type", ""))
        if clean == body:
            # The recorded digest is of the request as it was sent, which no committed body can reproduce
            # where the capture pinned part of it (a per-run directory, a tab name); nothing pairs by it.
            continue
        entry["body"] = base64.b64encode(clean).decode("ascii")
        entry["request_sha256"] = transcript.digest(
            entry["method"], entry["path"], clean
        )
    clean_document = _document(document)
    return raw if json.loads(clean_document) == json.loads(raw) else clean_document


def _cassette(path: Path) -> bytes:
    raw = path.read_bytes()
    document = json.loads(raw)
    items = [normalise_response(item) for item in document["interactions"]]
    if items == document["interactions"]:
        return raw
    return _document({**document, "interactions": items})


#: The gzip header flags: a stored file name, a comment (the text a header can hold), extra fields, a header CRC.
_FEXTRA, _FNAME, _FCOMMENT, _FHCRC = 0x04, 0x08, 0x10, 0x02
_CHUNK = 1 << 20
#: The longest file name or comment a member header may store: the archives are written with neither.
_MAX_HEADER_TEXT = 64 << 10


def _members(path: Path) -> Iterator[tuple[int, bytes]]:
    """``(flags, decompressed chunk)`` through every member of a gzip file, streaming: ``gzip`` reads every member
    but reports no header, and a later member's header can hold a name as well as the first's."""
    with path.open("rb") as source:
        pending = b""

        header = bytearray()

        def take(count: int) -> bytes:
            nonlocal pending
            while len(pending) < count and (more := source.read(_CHUNK)):
                pending += more
            if len(pending) < count:
                raise EOFError(f"{path}: truncated gzip header")
            head, pending = pending[:count], pending[count:]
            header.extend(head)
            return head

        def take_text() -> None:
            nonlocal pending
            while (end := pending.find(b"\0", 0, _MAX_HEADER_TEXT + 1)) < 0:
                if len(pending) > _MAX_HEADER_TEXT:
                    raise ValueError(
                        f"{path}: a gzip header text is longer than {_MAX_HEADER_TEXT} bytes"
                    )
                more = source.read(_CHUNK)
                if not more:
                    raise EOFError(f"{path}: truncated gzip header")
                pending += more
            take(end + 1)

        while True:
            if not pending and not (pending := source.read(_CHUNK)):
                return
            header.clear()
            fixed = take(10)
            # A member is refused, never rewritten into a valid one, where any part of it is damaged: its
            # magic, its method, reserved flags, the header's own checksum, or the payload's CRC and length.
            if fixed[:2] != b"\x1f\x8b" or fixed[2] != 8 or fixed[3] & 0xE0:
                raise ValueError(f"{path}: not a well-formed gzip member")
            flags = fixed[3]
            if flags & _FEXTRA:
                take(int.from_bytes(take(2), "little"))
            if flags & _FNAME:
                take_text()
            if flags & _FCOMMENT:
                take_text()
            if flags & _FHCRC:
                stated = int.from_bytes(take(2), "little")
                if zlib.crc32(bytes(header[:-2])) & 0xFFFF != stated:
                    raise ValueError(f"{path}: a gzip member header fails its checksum")
            # Announced before any content, so a member that inflates to nothing still reports its header.
            yield flags, b""
            inflate = zlib.decompressobj(-zlib.MAX_WBITS)
            crc, size = 0, 0
            while not inflate.eof:
                data = pending or source.read(_CHUNK)
                pending = b""
                if not data:
                    raise EOFError(f"{path}: truncated gzip member")
                # Inflated a bounded chunk at a time: a member compresses arbitrarily well. A full chunk can leave
                # output inside the inflater with no input left, so the loop goes on until a call yields less.
                while True:
                    chunk = inflate.decompress(data, _CHUNK)
                    data = inflate.unconsumed_tail
                    crc, size = zlib.crc32(chunk, crc), size + len(chunk)
                    if chunk:
                        yield flags, chunk
                    if inflate.eof or (not data and len(chunk) < _CHUNK):
                        break
            pending = inflate.unused_data
            trailer = take(8)
            # A damaged member is refused, never rewritten into a valid one that hides the damage.
            if trailer != (
                crc.to_bytes(4, "little") + (size & 0xFFFFFFFF).to_bytes(4, "little")
            ):
                raise ValueError(f"{path}: a gzip member fails its CRC or length check")


#: The longest line an archive may hold: one export, which the server would refuse far below this.
_MAX_LINE = 256 << 20


def _lines(chunks: Iterator[bytes]) -> Iterator[bytes]:
    """The lines of a stream, each held whole once (it is normalised as one export) and none past ``_MAX_LINE``."""
    pending = bytearray()
    for chunk in chunks:
        start = 0
        while True:
            end = chunk.find(b"\n", start)
            piece = chunk[start:] if end < 0 else chunk[start : end + 1]
            if len(pending) + len(piece) > _MAX_LINE:
                raise ValueError(f"an archive line is longer than {_MAX_LINE} bytes")
            pending += piece
            if end < 0:
                break
            yield bytes(pending)
            pending.clear()
            start = end + 1
    if pending:
        yield bytes(pending)


def _archive(path: Path) -> Path | None:
    """One gzip archive of JSON lines normalised, streaming, into a file beside it; that file where anything
    changed, else ``None``. A member header storing a name, a comment or extra fields is a change too: the
    rewrite stores none of them."""
    stored_text = False

    def chunks() -> Iterator[bytes]:
        nonlocal stored_text
        for flags, chunk in _members(path):
            stored_text = stored_text or bool(flags & (_FEXTRA | _FNAME | _FCOMMENT))
            yield chunk

    changed = False
    handle, target = tempfile.mkstemp(dir=path.parent, suffix=".tmp")
    try:
        with os.fdopen(handle, "wb") as sink:
            with gzip.GzipFile(filename="", mode="wb", fileobj=sink, mtime=0) as out:
                for line in _lines(chunks()):
                    clean = normalise_body(line, "")
                    changed = changed or clean != line
                    out.write(clean)
    except BaseException:
        os.unlink(target)
        raise
    if changed or stored_text:
        return Path(target)
    os.unlink(target)
    return None


Normaliser = Callable[[Path], bytes]


def targets() -> list[tuple[Path, Normaliser]]:
    """Every committed recording but the archives."""
    exports: list[tuple[Path, Normaliser]] = []
    transcripts: list[tuple[Path, Normaliser]] = []
    for path in sorted(FIXTURES.rglob("*")):
        if not path.is_file():
            continue
        if path.suffix in (".pb", ".json") and path.name.split("-", 1)[0] in (
            "req",
            "logs",
            "metrics",
        ):
            exports.append((path, _export))
        elif path.name == transcript.FILENAME:
            transcripts.append((path, _transcript))
    # A suite's cassettes and the per-release ones the matrix replays (``cassettes@<version>``).
    cassettes: list[tuple[Path, Normaliser]] = [
        (path, _cassette)
        for path in sorted(CASSETTES.rglob("*.json"))
        if path.parent.name == "cassettes" or path.parent.name.startswith("cassettes@")
    ]
    return transcripts + exports + cassettes


def main(argv: list[str]) -> None:
    # Validated before anything is read: an unknown argument (`--help` included) must never fall through to
    # rewriting the corpus.
    if any(arg != "--check" for arg in argv):
        raise SystemExit("usage: python -m harness normalise [--check]")
    check = "--check" in argv
    # Every file is normalised before any is written, archives into files beside them: a refusal or an unreadable
    # archive part-way leaves the tree as it was.
    pending: list[tuple[Path, bytes]] = []
    for path, normalise in targets():
        raw = path.read_bytes()
        clean = normalise(path)
        if clean != raw:
            pending.append((path, clean))
    rewritten: list[tuple[Path, Path]] = []
    try:
        for path in sorted(ARCHIVES.glob("*.jsonl.gz")):
            if (target := _archive(path)) is not None:
                rewritten.append((path, target))
    except BaseException:
        for _, target in rewritten:
            target.unlink()
        raise
    stale = [path.relative_to(REPO) for path, _ in pending] + [
        path.relative_to(REPO) for path, _ in rewritten
    ]
    if check:
        for _, target in rewritten:
            target.unlink()
    else:
        for path, clean in pending:
            path.write_bytes(clean)
        for path, target in rewritten:
            os.replace(target, path)
    verb = "would change" if check else "normalised"
    print(f"[normalise] {len(stale)} file(s) {verb}")
    for path in stale:
        print(f"  {path}")
    if check and stale:
        sys.exit(1)
