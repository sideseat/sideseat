"""OTLP trace generation and sending utilities."""

import json
import time
import uuid
from typing import Any
from urllib.request import Request, urlopen

from ..config import OTEL_BASE
from ..logging import log_info


def generate_trace_id() -> str:
    """Generate a random 32-character hex trace ID."""
    return uuid.uuid4().hex + uuid.uuid4().hex[:16]


def generate_span_id() -> str:
    """Generate a random 16-character hex span ID."""
    return uuid.uuid4().hex[:16]


def create_otlp_trace(
    trace_id: str,
    spans: list[dict[str, Any]],
    service_name: str = "e2e-test-service",
    framework: str | None = None,
) -> dict[str, Any]:
    """Create an OTLP ExportTraceServiceRequest JSON payload.

    Args:
        trace_id: The trace ID for all spans
        spans: List of span dictionaries with keys:
            - span_id: Span ID
            - parent_span_id: Optional parent span ID
            - name: Span name
            - kind: Span kind (1=INTERNAL, 2=SERVER, 3=CLIENT, 4=PRODUCER, 5=CONSUMER)
            - start_time_ns: Start time in nanoseconds
            - end_time_ns: End time in nanoseconds
            - attributes: Optional list of OTLP attribute dicts
            - status: Optional status dict with 'code' key
        service_name: Service name for the resource
        framework: Optional framework name to add as attribute

    Returns:
        OTLP ExportTraceServiceRequest dict
    """
    otlp_spans = []
    for span in spans:
        otlp_span = {
            "traceId": span.get("trace_id", trace_id),
            "spanId": span["span_id"],
            "name": span["name"],
            "kind": span.get("kind", 1),  # INTERNAL
            "startTimeUnixNano": str(span["start_time_ns"]),
            "endTimeUnixNano": str(span["end_time_ns"]),
            "attributes": span.get("attributes", []),
            "status": span.get("status", {"code": 1}),  # OK
        }
        if span.get("parent_span_id"):
            otlp_span["parentSpanId"] = span["parent_span_id"]
        if span.get("events"):
            otlp_span["events"] = span["events"]
        otlp_spans.append(otlp_span)

    resource_attrs = [
        {"key": "service.name", "value": {"stringValue": service_name}},
        {"key": "service.version", "value": {"stringValue": "1.0.0-e2e"}},
    ]

    if framework:
        resource_attrs.append(
            {"key": "telemetry.sdk.name", "value": {"stringValue": framework}}
        )

    return {
        "resourceSpans": [
            {
                "resource": {"attributes": resource_attrs},
                "scopeSpans": [
                    {
                        "scope": {"name": "e2e-test-scope", "version": "1.0.0"},
                        "spans": otlp_spans,
                    }
                ],
            }
        ]
    }


def send_otlp_traces_http(
    payload: dict[str, Any],
    endpoint: str | None = None,
    content_type: str = "application/json",
) -> tuple[bool, int]:
    """Send OTLP traces via HTTP POST.

    Args:
        payload: OTLP ExportTraceServiceRequest dict
        endpoint: Optional custom endpoint URL
        content_type: Content-Type header (application/json or application/x-protobuf)

    Returns:
        Tuple of (success, status_code)
    """
    url = endpoint or f"{OTEL_BASE}/v1/traces"
    try:
        data = json.dumps(payload).encode("utf-8")
        req = Request(
            url,
            data=data,
            headers={"Content-Type": content_type},
            method="POST",
        )
        with urlopen(req, timeout=30) as response:
            return (response.status == 200, response.status)
    except Exception as e:
        log_info(f"Failed to send OTLP trace: {e}")
        return (False, 0)


def create_simple_trace(
    service_name: str = "e2e-test-service",
    span_count: int = 1,
    with_genai: bool = False,
    session_id: str | None = None,
    user_id: str | None = None,
    with_error: bool = False,
) -> tuple[str, dict[str, Any]]:
    """Create a simple trace with specified number of spans.

    Args:
        service_name: Service name for the trace
        span_count: Number of spans to create
        with_genai: Add GenAI attributes to spans
        session_id: Optional session ID to associate with trace
        user_id: Optional user ID to associate with trace
        with_error: Mark trace as having an error (status code 2)

    Returns:
        Tuple of (trace_id, otlp_payload)
    """
    trace_id = generate_trace_id()
    now_ns = int(time.time() * 1_000_000_000)

    spans = []
    root_span_id = generate_span_id()

    # Root span
    root_attrs = [{"key": "e2e.test", "value": {"boolValue": True}}]
    if with_genai:
        root_attrs.extend(
            [
                {"key": "gen_ai.request.model", "value": {"stringValue": "gpt-4-test"}},
                {"key": "gen_ai.usage.input_tokens", "value": {"intValue": "100"}},
                {"key": "gen_ai.usage.output_tokens", "value": {"intValue": "200"}},
            ]
        )
    if session_id:
        root_attrs.append({"key": "session.id", "value": {"stringValue": session_id}})
    if user_id:
        root_attrs.append({"key": "user.id", "value": {"stringValue": user_id}})

    # Status code: 1 = OK, 2 = ERROR
    root_status = {"code": 2, "message": "Test error"} if with_error else {"code": 1}

    spans.append(
        {
            "trace_id": trace_id,
            "span_id": root_span_id,
            "parent_span_id": None,
            "name": "root-span",
            "kind": 1,
            "start_time_ns": now_ns,
            "end_time_ns": now_ns + 1_000_000_000,
            "attributes": root_attrs,
            "status": root_status,
        }
    )

    # Child spans
    for i in range(1, span_count):
        child_attrs = [{"key": "span.index", "value": {"intValue": str(i)}}]
        if session_id:
            child_attrs.append(
                {"key": "session.id", "value": {"stringValue": session_id}}
            )
        spans.append(
            {
                "trace_id": trace_id,
                "span_id": generate_span_id(),
                "parent_span_id": root_span_id,
                "name": f"child-span-{i}",
                "kind": 1,
                "start_time_ns": now_ns + (i * 100_000_000),
                "end_time_ns": now_ns + ((i + 1) * 100_000_000),
                "attributes": child_attrs,
                "status": {"code": 1},
            }
        )

    payload = create_otlp_trace(trace_id, spans, service_name)
    return (trace_id, payload)


def create_batch_traces(
    count: int,
    service_name: str = "e2e-batch-service",
    spans_per_trace: int = 3,
) -> list[tuple[str, dict[str, Any]]]:
    """Create multiple traces for batch testing.

    Args:
        count: Number of traces to create
        service_name: Service name for all traces
        spans_per_trace: Number of spans per trace

    Returns:
        List of (trace_id, otlp_payload) tuples
    """
    traces = []
    for _ in range(count):
        trace_id, payload = create_simple_trace(service_name, spans_per_trace)
        traces.append((trace_id, payload))
    return traces
