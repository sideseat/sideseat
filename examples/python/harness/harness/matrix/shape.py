"""The telemetry shape of a capture: what a rule can match on, with every value that varies run to run removed.

Two releases emit the same format exactly when their shapes are equal, which is what lets the census say
a release with no fixture of its own is covered by one that has the same shape. A shape keeps:

- per resource: its attribute keys;
- per span: instrumentation scope name, span name, attribute keys, event names with their attribute keys;
- per log record: scope name, event name, attribute keys, the body's structure;
- for every string value that parses as JSON: its structure (member names, element shapes, value kinds);
- every string value that reads as a lower-case identifier with no digits (``chat``, ``tool``,
  ``execute_tool``, ``gen_ai.choice``): those are the discriminators predicates compare against, while
  free text, ids, model names and timestamps all fail the test.

Numeric segments of keys (``llm.input_messages.3.message.role``) are folded, because an indexed family is
one format however long the conversation. Other values, ids, timestamps and scope versions are dropped:
they differ between two runs of one release or between two releases of one format. Element order and
multiplicity inside arrays are not kept either, so a shape is an empirical telemetry class rather than
a proof that two releases are interchangeable.
"""

from __future__ import annotations

import hashlib
import json
import re
from collections.abc import Iterable, Iterator
from pathlib import Path
from typing import Any

_INDEX = re.compile(r"(?<=\.)\d+(?=\.|$)")
_DISCRIMINATOR = re.compile(r"^[a-z][a-z_.\-]{0,39}$")
_DEPTH = 8


def skeleton(value: Any, depth: int = 0) -> str:
    """A JSON value's structure: member names and kinds, arrays as the union of their elements."""
    if depth > _DEPTH:
        return "…"
    if isinstance(value, dict):
        inner = ",".join(
            f"{k}:{skeleton(v, depth + 1)}" for k, v in sorted(value.items())
        )
        return "{" + inner + "}"
    if isinstance(value, list):
        members = sorted({skeleton(v, depth + 1) for v in value})
        return "[" + "|".join(members) + "]"
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, (int, float)):
        return "num"
    if value is None:
        return "null"
    return "str"


def _key(key: str) -> str:
    return _INDEX.sub("#", key)


def _value_shape(value: Any) -> str:
    """An attribute's kind, and for a JSON string its structure."""
    if isinstance(value, str):
        text = value.strip()
        if text[:1] in "[{":
            try:
                return "json" + skeleton(json.loads(text))
            except ValueError:
                return "str"
        return f"'{text}'" if _DISCRIMINATOR.match(text) else "str"
    return skeleton(value)


def _any_value(value: Any) -> Any:
    """An OTLP ``AnyValue`` as plain Python."""
    kind = value.WhichOneof("value")
    if kind is None:
        return None
    if kind == "array_value":
        return [_any_value(v) for v in value.array_value.values]
    if kind == "kvlist_value":
        return {kv.key: _any_value(kv.value) for kv in value.kvlist_value.values}
    if kind == "bytes_value":
        return "<bytes>"
    return getattr(value, kind)


def _attributes(attributes: Iterable[Any]) -> str:
    return ";".join(
        sorted(
            {
                f"{_key(kv.key)}={_value_shape(_any_value(kv.value))}"
                for kv in attributes
            }
        )
    )


def _load(path: Path, message: Any) -> Any:
    raw = path.read_bytes()
    if path.suffix == ".json":
        from google.protobuf import json_format

        # OTLP/JSON writes ids in hex where the protobuf mapping expects base64; ids are not shape.
        document = json.loads(raw)
        _drop_ids(document)
        return json_format.ParseDict(document, message, ignore_unknown_fields=True)
    message.ParseFromString(raw)
    return message


def _drop_ids(node: Any) -> None:
    if isinstance(node, dict):
        for key in (
            "traceId",
            "spanId",
            "parentSpanId",
            "trace_id",
            "span_id",
            "parent_span_id",
        ):
            node.pop(key, None)
        for child in node.values():
            _drop_ids(child)
    elif isinstance(node, list):
        for child in node:
            _drop_ids(child)


def _span_lines(path: Path) -> Iterator[str]:
    from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
        ExportTraceServiceRequest,
    )

    request = _load(path, ExportTraceServiceRequest())
    for resource in request.resource_spans:
        yield "resource " + ";".join(
            sorted({_key(kv.key) for kv in resource.resource.attributes})
        )
        for scoped in resource.scope_spans:
            scope = scoped.scope.name
            for span in scoped.spans:
                events = " ".join(
                    sorted(
                        {f"{e.name}({_attributes(e.attributes)})" for e in span.events}
                    )
                )
                name = _INDEX.sub("#", re.sub(r"\d+", "#", span.name))
                yield f"span {scope} | {name} | {_attributes(span.attributes)} | {events}"


def _log_lines(path: Path) -> Iterator[str]:
    from opentelemetry.proto.collector.logs.v1.logs_service_pb2 import (
        ExportLogsServiceRequest,
    )

    request = _load(path, ExportLogsServiceRequest())
    for resource in request.resource_logs:
        for scoped in resource.scope_logs:
            scope = scoped.scope.name
            for record in scoped.log_records:
                body = _value_shape(_any_value(record.body))
                yield f"log {scope} | {record.event_name} | {_attributes(record.attributes)} | {body}"


def shape(directory: Path) -> list[str]:
    """The sorted, de-duplicated shape lines of every export in a fixture directory."""
    lines: set[str] = set()
    for path in sorted(directory.iterdir()):
        if path.name.startswith("req-"):
            lines.update(_span_lines(path))
        elif path.name.startswith("logs-"):
            lines.update(_log_lines(path))
    return sorted(lines)


def digest(lines: list[str]) -> str:
    return hashlib.sha256("\n".join(lines).encode()).hexdigest()[:16]
