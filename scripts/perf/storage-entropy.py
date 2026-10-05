#!/usr/bin/env python3
"""Measure how compactly the captured trace corpus can be stored, layer by layer.

Reproduces the numbers in docs/engineering/compact-storage.md. Offline: it reads the committed OTLP
fixtures and nothing else.

    uv run --no-project --with zstandard --with opentelemetry-proto==1.45.0 \\
        python scripts/perf/storage-entropy.py [--exclude codex] [--only codex]

A tenant is one framework's native captures: one application run many times, which is what a hosted
user is. The SDK mode is left out because it replays the same model responses and would flatter
deduplication.
"""

from __future__ import annotations

import collections
import glob
import hashlib
import json
import os
import re
import sys

import zstandard as zstd
from opentelemetry.proto.collector.logs.v1.logs_service_pb2 import (
    ExportLogsServiceRequest,
)
from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
    ExportTraceServiceRequest,
)

ROOT = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..",
    "..",
    "server/tests/fixtures/messages",
)
BASE64 = re.compile(r"[A-Za-z0-9+/]{256,}={0,2}")
INLINE = 24  # leaves shorter than this stay inline; a reference would cost as much
LARGE = 96  # attribute values longer than this go to the content store


def anyval(v):
    kind = v.WhichOneof("value")
    if kind == "array_value":
        return [anyval(x) for x in v.array_value.values]
    if kind == "kvlist_value":
        return {x.key: anyval(x.value) for x in v.kvlist_value.values}
    return getattr(v, kind) if kind else None


def leaves(value, out, depth=0):
    """Split a value into a JSON skeleton and its string leaves, parsing JSON nested in strings."""
    if isinstance(value, str):
        if len(value) > 1 and value[0] in "[{" and depth < 6:
            try:
                return leaves(json.loads(value), out, depth + 1)
            except ValueError:
                pass
        out.append(value)
        return "\x01"
    if isinstance(value, dict):
        return {k: leaves(v, out, depth) for k, v in value.items()}
    if isinstance(value, list):
        return [leaves(v, out, depth) for v in value]
    return value


def varint(n: int) -> bytes:
    out = bytearray()
    while True:
        b, n = n & 0x7F, n >> 7
        out.append(b | 0x80 if n else b)
        if not n:
            return bytes(out)


def selected(pattern: str) -> list[str]:
    """Native captures, minus the producers named by `--exclude a,b` or kept by `--only a,b`."""
    args = dict(zip(sys.argv[1::2], sys.argv[2::2]))
    exclude = set(filter(None, args.get("--exclude", "").split(",")))
    only = set(filter(None, args.get("--only", "").split(",")))
    files = sorted(glob.glob(f"{ROOT}/*/native/**/{pattern}", recursive=True))
    producer = lambda f: os.path.relpath(f, ROOT).split(os.sep)[0]  # noqa: E731
    return [
        f
        for f in files
        if producer(f) not in exclude and (not only or producer(f) in only)
    ]


def main() -> None:
    files = selected("req-*.pb")
    z19 = zstd.ZstdCompressor(level=19)
    spans = collections.Counter()
    raw = collections.Counter()
    traces: set[bytes] = set()
    meta = collections.defaultdict(bytearray)
    tenant = collections.defaultdict(lambda: {"leaf": [], "skel": [], "shapes": {}})
    for path in files:
        t = os.path.relpath(path, ROOT).split(os.sep)[0]
        data = open(path, "rb").read()
        raw[t] += len(data)
        request = ExportTraceServiceRequest()
        request.ParseFromString(data)
        for rs in request.resource_spans:
            for ss in rs.scope_spans:
                prev = 0
                for s in sorted(
                    ss.spans, key=lambda s: (s.trace_id, s.start_time_unix_nano)
                ):
                    spans[t] += 1
                    if s.trace_id not in traces:
                        traces.add(s.trace_id)
                        meta["trace_id"] += s.trace_id
                    meta["span_id"] += s.span_id
                    delta = (s.start_time_unix_nano - prev) // 1000
                    meta["start"] += varint((delta << 1) ^ (delta >> 63))
                    prev = s.start_time_unix_nano
                    meta["duration"] += varint(
                        (s.end_time_unix_nano - s.start_time_unix_nano) // 1000
                    )
                    attrs = [(a.key, anyval(a.value)) for a in s.attributes]
                    attrs += [
                        (f"event:{e.name}:{a.key}", anyval(a.value))
                        for e in s.events
                        for a in e.attributes
                    ]
                    # A span's *shape* - name, kind and its attribute key set - repeats across an app's runs, so it
                    # is one dictionary id per span rather than a key per attribute.
                    shape = (s.name, s.kind, s.status.code, tuple(k for k, _ in attrs))
                    shapes = tenant[t]["shapes"]
                    meta["shape"] += varint(shapes.setdefault(shape, len(shapes)))
                    for _, v in attrs:
                        if isinstance(v, (dict, list)) or (
                            isinstance(v, str) and len(v) > LARGE
                        ):
                            out: list[str] = []
                            tenant[t]["skel"].append(
                                json.dumps(
                                    leaves(v, out), separators=(",", ":")
                                ).encode()
                            )
                            tenant[t]["leaf"] += out
                        else:
                            meta["scalar"] += str(v).encode() + b"\x00"
    total = sum(spans.values())
    if not total:
        print("no trace requests selected")
        return
    acc = collections.Counter()
    for t, d in tenant.items():
        seen: set[bytes] = set()
        unique = []
        for leaf in d["leaf"]:
            media = sum(len(m) for m in BASE64.findall(leaf))
            if media:
                acc["media"] += media * 3 // 4
                leaf = BASE64.sub("\x02", leaf)
            b = leaf.encode()
            acc["leaf_all"] += len(b)
            if len(b) < INLINE:
                unique.append(b)
                continue
            digest = hashlib.blake2b(b, digest_size=8).digest()
            acc["refs"] += 5
            if digest not in seen:
                seen.add(digest)
                unique.append(b)
        skeletons = list(dict.fromkeys(d["skel"]))
        acc["refs"] += 3 * len(d["skel"])
        leaf_bytes = b"\x00".join(unique)
        acc["leaf_unique"] += len(leaf_bytes)
        acc["leaf_z"] += len(z19.compress(leaf_bytes)) if leaf_bytes else 0
        acc["skel_z"] += len(z19.compress(b"\x00".join(skeletons))) if skeletons else 0
        acc["shape_dict"] += len(z19.compress(repr(list(d["shapes"])).encode()))
    meta_z = {k: len(z19.compress(bytes(v))) for k, v in meta.items()}
    per = lambda n: n / total  # noqa: E731
    print(
        f"{len(files)} requests, {len(spans)} tenants, {len(traces)} traces, {total} spans"
    )
    print(f"raw OTLP protobuf                 {per(sum(raw.values())):8.0f} B/span")
    print(f"text leaves, all                  {per(acc['leaf_all']):8.0f} B/span")
    print(f"text leaves, deduplicated         {per(acc['leaf_unique']):8.0f} B/span")
    print(f"text leaves, dedup + zstd-19      {per(acc['leaf_z']):8.0f} B/span")
    print(f"JSON skeletons, dedup + zstd-19   {per(acc['skel_z']):8.1f} B/span")
    print(f"content references                {per(acc['refs']):8.1f} B/span")
    for k, v in sorted(meta_z.items(), key=lambda kv: -kv[1]):
        print(f"column {k:26s} {per(v):8.1f} B/span")
    print(f"shape dictionaries                {per(acc['shape_dict']):8.1f} B/span")
    text_total = (
        acc["leaf_z"]
        + acc["skel_z"]
        + acc["refs"]
        + sum(meta_z.values())
        + acc["shape_dict"]
    )
    print(f"TOTAL without media               {per(text_total):8.1f} B/span")
    print(
        f"media, decoded binary             {per(acc['media']):8.0f} B/span (blob store)"
    )


def measure_logs() -> None:
    """The same layers for log records: shape id, delta time, scalars, and content by reference."""
    files = selected("logs-*.pb")
    if not files:
        return
    z19 = zstd.ZstdCompressor(level=19)
    raw = records = 0
    meta = collections.defaultdict(bytearray)
    tenant = collections.defaultdict(lambda: {"leaf": [], "shapes": {}})
    for path in files:
        t = os.path.relpath(path, ROOT).split(os.sep)[0]
        data = open(path, "rb").read()
        raw += len(data)
        request = ExportLogsServiceRequest()
        request.ParseFromString(data)
        for rl in request.resource_logs:
            for sl in rl.scope_logs:
                prev = 0
                for r in sorted(sl.log_records, key=lambda r: r.time_unix_nano):
                    records += 1
                    stamp = r.time_unix_nano or r.observed_time_unix_nano
                    delta = (stamp - prev) // 1000
                    meta["time"] += varint((delta << 1) ^ (delta >> 63))
                    prev = stamp
                    if r.span_id:
                        meta["span_ref"] += r.span_id
                    attrs = [(a.key, anyval(a.value)) for a in r.attributes]
                    attrs.append(("body", anyval(r.body)))
                    shape = (
                        r.severity_number,
                        r.event_name,
                        tuple(k for k, _ in attrs),
                    )
                    shapes = tenant[t]["shapes"]
                    meta["shape"] += varint(shapes.setdefault(shape, len(shapes)))
                    for _, v in attrs:
                        if isinstance(v, (dict, list)) or (
                            isinstance(v, str) and len(v) > LARGE
                        ):
                            out: list[str] = []
                            leaves(v, out)
                            tenant[t]["leaf"] += out
                        else:
                            meta["scalar"] += str(v).encode() + b"\x00"
    content = refs = 0
    for d in tenant.values():
        seen: set[bytes] = set()
        unique = []
        for leaf in d["leaf"]:
            b = leaf.encode()
            if len(b) >= INLINE:
                refs += 5
                digest = hashlib.blake2b(b, digest_size=8).digest()
                if digest in seen:
                    continue
                seen.add(digest)
            unique.append(b)
        joined = b"\x00".join(unique)
        content += len(z19.compress(joined)) if joined else 0
        content += len(z19.compress(repr(list(d["shapes"])).encode()))
    columns = sum(len(z19.compress(bytes(v))) for v in meta.values())
    print(f"\n{len(files)} log exports, {records} records")
    print(f"raw OTLP protobuf                 {raw / records:8.0f} B/record")
    print(f"columns (time, span, shape, scalars) {columns / records:5.1f} B/record")
    print(
        f"content, dedup + zstd-19 + refs   {(content + refs) / records:8.1f} B/record"
    )
    print(
        f"TOTAL                             {(columns + content + refs) / records:8.1f} B/record"
    )


if __name__ == "__main__":
    measure_logs()
    sys.exit(main())
