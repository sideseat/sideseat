"""Proving the stored raw records are the bytes the producers sent.

Imported by `storage-footprint.py` (`--verify-raw`): the footprint run already has a loaded store and the
exports it was loaded from, so the cheapest place to check the authority is there. Kept in its own module
because the measurement script is long enough.
"""

from __future__ import annotations

import collections
from pathlib import Path


def log(message: str) -> None:
    print(f"[storage] {message}", flush=True)


def decode_ssr1(record: bytes, media) -> bytes:
    """The received body back from an SSR1 record; `media(hash_hex)` answers an object's decoded bytes."""
    import base64

    if record[:4] != b"SSR1":
        raise ValueError("not an SSR1 record")
    at = 5

    def varint() -> int:
        nonlocal at
        value, shift = 0, 0
        while True:
            byte = record[at]
            at += 1
            value |= (byte & 0x7F) << shift
            shift += 7
            if byte < 0x80:
                return value

    runs = []
    for _ in range(varint()):
        gap, length = varint(), varint()
        runs.append((gap, length, record[at : at + 32].hex()))
        at += 32
    body, cursor, out = record[at:], 0, bytearray()
    for gap, length, digest in runs:
        out += body[cursor : cursor + gap]
        cursor += gap
        text = base64.b64encode(media(digest))
        if len(text) != length:
            raise ValueError(
                f"media {digest} does not re-encode to {length} characters"
            )
        out += text
    out += body[cursor:]
    return bytes(out)


def verify_raw_embedded(
    work: Path, exports: list[dict], projects: dict[str, str]
) -> int:
    """Every stored raw record decodes to the exact bytes of an export its project was sent, and every trace
    export is stored. Media comes back from the file store, as a re-derivation would read it."""
    import duckdb

    sent = collections.defaultdict(set)
    for export in exports:
        if export["signal"] == "traces":
            sent[projects[export["tenant"]]].add(export["body"])
    files = work / "files"

    def media_of(project: str):
        def lookup(digest: str) -> bytes:
            return (files / project / digest[:2] / digest[2:4] / digest).read_bytes()

        return lookup

    connection = duckdb.connect(str(work / "duckdb/sideseat.duckdb"), read_only=True)
    rows = connection.execute(
        "SELECT project_id, record, origin FROM otel_raw QUALIFY ROW_NUMBER() OVER "
        "(PARTITION BY project_id, raw_id ORDER BY version DESC, rowid DESC) = 1"
    ).fetchall()
    connection.close()
    failures, stored = [], collections.defaultdict(set)
    for project, record, origin in rows:
        # Nothing is deleted while the corpus is ingested, and every fixture is storable, so every record is
        # the body that was received. One that is not means the corpus changed, and "byte for byte" would be
        # measuring something else - so it is a failure here rather than a silently weaker check.
        if origin != "received":
            failures.append(
                f"{project}: a record is {origin!r} rather than the received body"
            )
            continue
        try:
            body = decode_ssr1(bytes(record), media_of(project))
        except (ValueError, OSError) as error:
            failures.append(f"{project}: {error}")
            continue
        if body not in sent[project]:
            failures.append(
                f"{project}: a record decodes to bytes no export of the project had"
            )
        stored[project].add(body)
    for project, bodies in sent.items():
        missing = len(bodies - stored[project])
        if missing:
            failures.append(f"{project}: {missing} trace exports have no raw record")
    for failure in failures[:20]:
        log(f"FAIL raw: {failure}")
    log(
        f"raw round trip: {len(rows)} records, {sum(len(v) for v in sent.values())} distinct exports, "
        f"{len(failures)} failures"
    )
    return 1 if failures else 0
