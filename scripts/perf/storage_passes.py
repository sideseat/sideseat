"""The storage corpus replayed as more telemetry, for what one more item costs on a live server.

A pass after the first sends every export again as new telemetry, as `server/tests/storage_gate.rs`
does in process: every trace and span id rewritten by the pass, the same length, so it is stored rather than
recognised as a re-send. Its instants move a second a pass, back, where the in-process gate moves them a day on:
ClickHouse partitions by month, and a day a pass would put ten passes in a partition three did not reach on some
dates, charging the difference a partition's fixed cost; a second keeps every count in pass one's partitions, and
inside the retention horizon the distributed corpus is rebased to. A rewritten export is sent as protobuf. A metric
export is replayed with its instants moved, which makes every point a new one, and its exemplars' trace and span ids
rewritten as the spans they name are. The metric
figure is not read from these passes - a million points of the derived load judge it - but the passes must grow the
metric tables too, or a table that grows with points would never show in the growth audit.
"""

from __future__ import annotations

import base64
import json

SHIFT_NS = 10**9
# OTLP/JSON spells ids as hex, where the protobuf JSON mapping reads base64.
ID_FIELDS = {
    "traceId",
    "spanId",
    "parentSpanId",
    "trace_id",
    "span_id",
    "parent_span_id",
}


def replayed(exports: list[dict], passes: int) -> list[dict]:
    """`exports`, then each later pass's exports, corpus order within a pass."""
    out = list(exports)
    for index in range(1, passes):
        out += [for_pass(e, index) for e in exports]
    return out


def for_pass(export: dict, index: int) -> dict:
    from storage_rebase import decoded, instants

    message = parsed_json(export) if export["json"] else decoded(export)
    key = ((index * 0x9E37_79B9_7F4A_7C15) % 2**64).to_bytes(8, "big")
    for holder, field in ids(export["signal"], message):
        value = getattr(holder, field)
        if value:
            setattr(holder, field, bytes(b ^ key[i % 8] for i, b in enumerate(value)))
    for holder, field in instants(export["signal"], message):
        if getattr(holder, field):
            setattr(holder, field, getattr(holder, field) - index * SHIFT_NS)
    body = message.SerializeToString()
    return {**export, "body": body, "json": False, "protobuf_bytes": len(body)}


def parsed_json(export: dict):
    from google.protobuf import json_format
    from storage_rebase import decoded

    def unhex(node):
        if isinstance(node, dict):
            return {
                k: base64.b64encode(bytes.fromhex(v)).decode()
                if k in ID_FIELDS and isinstance(v, str)
                else unhex(v)
                for k, v in node.items()
            }
        if isinstance(node, list):
            return [unhex(v) for v in node]
        return node

    message = decoded({**export, "body": b"", "json": False})
    json_format.ParseDict(unhex(json.loads(export["body"])), message)
    return message


def ids(signal_name: str, message):
    """Every (message, field) of `message` holding a trace or span id."""
    if signal_name == "traces":
        for rs in message.resource_spans:
            for ss in rs.scope_spans:
                for span in ss.spans:
                    yield span, "trace_id"
                    yield span, "span_id"
                    yield span, "parent_span_id"
                    for link in span.links:
                        yield link, "trace_id"
                        yield link, "span_id"
    elif signal_name == "logs":
        for rl in message.resource_logs:
            for sl in rl.scope_logs:
                for record in sl.log_records:
                    yield record, "trace_id"
                    yield record, "span_id"
    elif signal_name == "metrics":
        # A point's exemplars name the span they were sampled in, which a later pass rewrites like any other.
        for rm in message.resource_metrics:
            for sm in rm.scope_metrics:
                for metric in sm.metrics:
                    kind = metric.WhichOneof("data")
                    if kind is None:
                        continue
                    for point in getattr(metric, kind).data_points:
                        # A summary point has no exemplars.
                        for exemplar in getattr(point, "exemplars", []):
                            yield exemplar, "trace_id"
                            yield exemplar, "span_id"


def self_test(exports: list[dict]) -> None:
    """A later pass keeps every export's items and id lengths, and changes every nonzero id and instant."""
    from storage_rebase import decoded, instants

    checked = 0
    for export in exports:
        before = parsed_json(export) if export["json"] else decoded(export)
        after = decoded(for_pass(export, 1))
        old = [getattr(h, f) for h, f in ids(export["signal"], before)]
        new = [getattr(h, f) for h, f in ids(export["signal"], after)]
        assert [len(v) for v in old] == [len(v) for v in new], (
            f"{export['path']}: an id changed length"
        )
        assert all(a != b for a, b in zip(old, new) if a), (
            f"{export['path']}: an id was kept"
        )
        shifts = {
            getattr(h, f) - getattr(g, f2)
            for (g, f2), (h, f) in zip(
                instants(export["signal"], after), instants(export["signal"], before)
            )
            if getattr(h, f)
        }
        assert shifts <= {SHIFT_NS}, f"{export['path']}: instants moved by {shifts}"
        checked += 1
    print(
        f"[storage] pass self-test: {checked} exports rewritten",
        flush=True,
    )
