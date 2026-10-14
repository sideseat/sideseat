"""Moving a storage corpus in time without changing anything else about it.

The distributed footprint replays exports captured months ago into ClickHouse, whose tables drop rows past
their TTL by the rows' own timestamps; `rebased` brings each tenant's exports to the present first.
"""

from __future__ import annotations

import re
import time
from pathlib import Path


# An instant in an OTLP/JSON export: a `...TimeUnixNano` field (or its snake_case spelling), as a string or a number.
JSON_INSTANT = re.compile(
    rb'(?<!\\)("(?:[A-Za-z]*[tT]imeUnixNano|[a-z_]*time_unix_nano)"\s*:\s*)("?)(\d+)("?)'
)


def rebased(exports: list[dict], now_ns: int) -> list[dict]:
    """`exports` moved in time, a tenant at a time: each tenant's latest instant becomes a day before `now_ns`,
    and every instant of that tenant moves by the same amount, so the offsets within and between its exports -
    the redeliveries and repeated spans a project's storage depends on - are kept.

    ClickHouse drops, in a merge soon after it writes them, rows older than its tables' TTL by their own
    timestamp, and the server reports such records rejected; the corpus is captured over months and the synthetic
    fixtures are dated 2025-01-01, so without this the distributed footprint would measure only the exports young
    enough to keep. The distributed run alone: the embedded gate's bytes are held to the corpus as committed.

    Only the instants change. An OTLP/JSON export is shifted in its own text - re-encoding it through the
    protobuf JSON printer writes enum names and base64 ids, which OTLP/JSON is not (`self_test` holds this) - and
    a protobuf export through its message.
    """
    day = 86_400 * 10**9
    spans = []
    for export in exports:
        values = [value for value in export_instants(export) if value]
        spans.append((min(values), max(values)) if values else (0, 0))
    earliest: dict[str, int] = {}
    latest: dict[str, int] = {}
    for export, (low, high) in zip(exports, spans):
        if high:
            tenant = export["tenant"]
            earliest[tenant] = min(earliest.get(tenant, high), low)
            latest[tenant] = max(latest.get(tenant, 0), high)
    out = []
    for export, (_, high) in zip(exports, spans):
        if not high:
            out.append(export)
            continue
        tenant = export["tenant"]
        # A tenant captured over longer than the horizon - the synthetic fixtures, two years of independent
        # scenarios - moves an export at a time, or its oldest exports would still fall past it.
        anchor = (
            latest[tenant] if latest[tenant] - earliest[tenant] <= 80 * day else high
        )
        out.append({**export, "body": shifted(export, now_ns - day - anchor)})
    return out


def export_instants(export: dict) -> list[int]:
    if export["json"]:
        return [int(match[3]) for match in JSON_INSTANT.finditer(export["body"])]
    message = decoded(export)
    return [
        getattr(holder, field) for holder, field in instants(export["signal"], message)
    ]


def shifted(export: dict, shift: int) -> bytes:
    """The export's body with every nonzero instant moved by `shift`, and nothing else changed."""
    if export["json"]:

        def move(match: re.Match) -> bytes:
            value = int(match[3])
            moved = value + shift if value else value
            return match[1] + match[2] + str(moved).encode() + match[4]

        return JSON_INSTANT.sub(move, export["body"])
    message = decoded(export)
    for holder, field in instants(export["signal"], message):
        if getattr(holder, field):
            setattr(holder, field, getattr(holder, field) + shift)
    return message.SerializeToString()


def decoded(export: dict):
    """A protobuf export, decoded."""
    from opentelemetry.proto.collector.logs.v1.logs_service_pb2 import (
        ExportLogsServiceRequest,
    )
    from opentelemetry.proto.collector.metrics.v1.metrics_service_pb2 import (
        ExportMetricsServiceRequest,
    )
    from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
        ExportTraceServiceRequest,
    )

    types = {
        "traces": ExportTraceServiceRequest,
        "logs": ExportLogsServiceRequest,
        "metrics": ExportMetricsServiceRequest,
    }
    message = types[export["signal"]]()
    message.ParseFromString(export["body"])
    return message


def self_test(corpus: Path) -> None:
    """`rebased` changes an OTLP/JSON export's instants and not one other byte, finds every instant its parsed
    message holds, and moves each by one amount; a protobuf export decodes to its message apart from its
    instants. Every JSON export of the corpus, and one protobuf one, are checked."""
    from google.protobuf import json_format

    signals = {"req": "traces", "logs": "logs", "metrics": "metrics"}
    masked = lambda text: JSON_INSTANT.sub(lambda m: m[1] + m[2] + b"0" + m[4], text)  # noqa: E731
    now_ns = time.time_ns()
    checked = 0
    for path in sorted(corpus.rglob("*-*.json")):
        signal = signals.get(path.name.split("-", 1)[0])
        if signal is None:
            continue
        body = path.read_bytes()
        export = {"signal": signal, "json": True, "body": body, "tenant": "t"}
        [moved] = rebased([export], now_ns)
        before = [int(m[3]) for m in JSON_INSTANT.finditer(body)]
        after = [int(m[3]) for m in JSON_INSTANT.finditer(moved["body"])]
        parsed = json_format.Parse(body, decoded({**export, "body": b""}))
        held = [getattr(holder, field) for holder, field in instants(signal, parsed)]
        assert sorted(filter(None, before)) == sorted(filter(None, held)), (
            f"{path}: the text's instants are not the message's"
        )
        assert masked(body) == masked(moved["body"]), (
            f"{path}: a byte other than an instant changed"
        )
        shifts = {b - a for a, b in zip(before, after) if a}
        assert len(shifts) <= 1 and 0 not in shifts, (
            f"{path}: one shift for every instant: {shifts}"
        )
        checked += 1
    assert checked, "the corpus has JSON exports to check"
    for path in sorted(corpus.rglob("req-*.pb"))[:1]:
        proto = {
            "signal": "traces",
            "json": False,
            "body": path.read_bytes(),
            "tenant": "t",
        }
        [moved] = rebased([proto], now_ns)
        original, shifted_message = decoded(proto), decoded(moved)
        for message in (original, shifted_message):
            for holder, field in instants("traces", message):
                setattr(holder, field, 0)
        assert original == shifted_message, (
            f"{path}: a field other than an instant changed"
        )
    print(
        f"[storage] rebase self-test: {checked} JSON exports moved, nothing else",
        flush=True,
    )


def instants(signal_name: str, message):
    """Every (message, field) of `message` holding an instant in Unix nanoseconds."""
    if signal_name == "traces":
        for rs in message.resource_spans:
            for ss in rs.scope_spans:
                for span in ss.spans:
                    yield span, "start_time_unix_nano"
                    yield span, "end_time_unix_nano"
                    for event in span.events:
                        yield event, "time_unix_nano"
    elif signal_name == "logs":
        for rl in message.resource_logs:
            for sl in rl.scope_logs:
                for record in sl.log_records:
                    yield record, "time_unix_nano"
                    yield record, "observed_time_unix_nano"
    else:
        for rm in message.resource_metrics:
            for sm in rm.scope_metrics:
                for metric in sm.metrics:
                    kind = metric.WhichOneof("data")
                    if not kind:
                        continue
                    for point in getattr(metric, kind).data_points:
                        yield point, "time_unix_nano"
                        if point.DESCRIPTOR.fields_by_name.get("start_time_unix_nano"):
                            yield point, "start_time_unix_nano"
                        for exemplar in getattr(point, "exemplars", []):
                            yield exemplar, "time_unix_nano"
