"""Value encoding for OTEL span export."""

import base64
import json
from datetime import date, datetime, timezone
from typing import Any

from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.trace import format_span_id, format_trace_id


def encode_value(value: Any) -> Any:
    """Encode value for JSON, converting binary to base64."""
    # Fast path for common primitives
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    if isinstance(value, memoryview):
        value = value.tobytes()
    if isinstance(value, (bytes, bytearray)):
        return base64.b64encode(value).decode("ascii")
    if isinstance(value, dict):
        return {k: encode_value(v) for k, v in value.items()}
    if isinstance(value, set):
        encoded = [encode_value(item) for item in value]
        try:
            return sorted(encoded)
        except TypeError:
            return encoded
    if isinstance(value, (list, tuple)):
        return [encode_value(item) for item in value]
    try:
        json.dumps(value)
        return value
    except (TypeError, OverflowError, ValueError):
        return f"<{type(value).__name__}>"


def _ns_to_iso8601(ns: int | None) -> str | None:
    """Convert nanoseconds to ISO8601 timestamp."""
    if ns is None:
        return None
    return datetime.fromtimestamp(ns / 1e9, tz=timezone.utc).isoformat()


def span_to_dict(span: ReadableSpan) -> dict[str, Any]:
    """Convert ReadableSpan to dict with encoded values."""
    ctx = span.context
    parent = span.parent

    attrs = {}
    if span.attributes:
        attrs = {k: encode_value(v) for k, v in dict(span.attributes).items()}

    resource_attrs = {}
    if span.resource and span.resource.attributes:
        resource_attrs = {k: encode_value(v) for k, v in dict(span.resource.attributes).items()}

    scope = getattr(span, "instrumentation_scope", None)
    scope_dict = None
    if scope:
        scope_dict = {
            "name": scope.name,
            "version": scope.version,
            "schema_url": scope.schema_url,
        }

    status = getattr(span, "status", None)
    status_dict = None
    if status:
        status_dict = {
            "status_code": str(status.status_code),
            "description": status.description,
        }

    events = []
    for e in span.events or []:
        event_attrs = (
            {k: encode_value(v) for k, v in dict(e.attributes).items()} if e.attributes else {}
        )
        events.append(
            {
                "name": e.name,
                "timestamp": _ns_to_iso8601(e.timestamp),
                "attributes": event_attrs,
            }
        )

    links = []
    for link in span.links or []:
        link_attrs = (
            {k: encode_value(v) for k, v in dict(link.attributes).items()}
            if link.attributes
            else {}
        )
        links.append(
            {
                "trace_id": format_trace_id(link.context.trace_id),
                "span_id": format_span_id(link.context.span_id),
                "attributes": link_attrs,
            }
        )

    return {
        "name": span.name,
        "trace_id": format_trace_id(ctx.trace_id),
        "span_id": format_span_id(ctx.span_id),
        "parent_span_id": format_span_id(parent.span_id) if parent else None,
        "kind": str(span.kind),
        "start_time": _ns_to_iso8601(span.start_time),
        "end_time": _ns_to_iso8601(span.end_time),
        "duration_ms": (
            (span.end_time - span.start_time) / 1e6 if span.start_time and span.end_time else None
        ),
        "attributes": attrs,
        "events": events,
        "links": links,
        "status": status_dict,
        "resource": resource_attrs,
        "scope": scope_dict,
    }
