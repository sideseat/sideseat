"""Value encoding and OTEL resource creation."""

import base64
import json
import logging
from datetime import date, datetime, timezone
from typing import Any

from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.trace import format_span_id, format_trace_id

logger = logging.getLogger("sideseat.telemetry.encoding")


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


def get_otel_resource(service_name: str, service_version: str) -> Resource:
    """Create OTEL resource with service info."""
    from sideseat._version import __version__ as sdk_version

    return Resource.create(
        {
            "service.name": service_name,
            "service.version": service_version,
            "telemetry.sdk.name": "sideseat",
            "telemetry.sdk.version": sdk_version,
            "telemetry.sdk.language": "python",
        }
    )


def patch_adk_tracing() -> bool:
    """Patch ADK tracing to preserve inline_data as base64 instead of stripping it.

    ADK's _build_llm_request_for_trace strips all parts with inline_data,
    losing multimodal content (images, PDFs) from telemetry. This patch
    base64-encodes the binary data so the actual content is preserved.
    """
    try:
        from google.adk.telemetry import tracing as adk_tracing  # type: ignore  # noqa: I001

        def _patched(llm_request: Any) -> dict[str, Any]:
            result = {
                "model": llm_request.model,
                "config": llm_request.config.model_dump(
                    exclude_none=True, exclude="response_schema"
                ),
                "contents": [],
            }
            for content in llm_request.contents:
                dumped_parts = []
                for part in content.parts:
                    if part.inline_data:
                        data = part.inline_data.data
                        dumped_parts.append(
                            {
                                "inline_data": {
                                    "mime_type": part.inline_data.mime_type,
                                    "data": base64.b64encode(data).decode("ascii") if data else "",
                                }
                            }
                        )
                    else:
                        dumped = part.model_dump(exclude_none=True)
                        if dumped:
                            dumped_parts.append(dumped)
                result["contents"].append(
                    {
                        "role": content.role,
                        "parts": dumped_parts,
                    }
                )
            return result

        adk_tracing._build_llm_request_for_trace = _patched
        logger.debug("Patched ADK tracing")
        return True
    except ImportError:
        logger.debug("Google ADK not installed")
        return False


def patch_strands_encoder() -> bool:
    """Patch Strands JSONEncoder for base64 binary encoding."""
    try:
        from strands.telemetry import tracer  # type: ignore[import-not-found]

        def _process_value(self: Any, value: Any) -> Any:
            return encode_value(value)

        tracer.JSONEncoder._process_value = _process_value
        logger.debug("Patched Strands encoder")
        return True
    except ImportError:
        logger.debug("Strands not installed")
        return False
