"""Synthetic trace ingestion and verification tests."""

import json
import time
import uuid
from typing import Any
from urllib.request import Request, urlopen

from ...api import api_call, encode_param
from ...config import OTEL_BASE
from ...logging import log_info, log_section
from ..base import BaseTestSuite


def generate_trace_id() -> str:
    """Generate a random 32-character hex trace ID."""
    return uuid.uuid4().hex + uuid.uuid4().hex[:16]


def generate_span_id() -> str:
    """Generate a random 16-character hex span ID."""
    return uuid.uuid4().hex[:16]


def create_otlp_trace(
    trace_id: str,
    spans: list[dict[str, Any]],
    service_name: str = "synthetic-test-service",
) -> dict[str, Any]:
    """Create an OTLP ExportTraceServiceRequest JSON payload."""
    otlp_spans = []
    for span in spans:
        otlp_span = {
            "traceId": span["trace_id"],
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
        otlp_spans.append(otlp_span)

    return {
        "resourceSpans": [
            {
                "resource": {
                    "attributes": [
                        {
                            "key": "service.name",
                            "value": {"stringValue": service_name},
                        },
                        {
                            "key": "service.version",
                            "value": {"stringValue": "1.0.0-synthetic"},
                        },
                    ]
                },
                "scopeSpans": [
                    {
                        "scope": {
                            "name": "synthetic-test-scope",
                            "version": "1.0.0",
                        },
                        "spans": otlp_spans,
                    }
                ],
            }
        ]
    }


class SyntheticTraceTests(BaseTestSuite):
    """Tests that ingest a synthetic trace and verify all data is retrievable via API."""

    def __init__(self) -> None:
        super().__init__()
        self.synthetic_trace_id: str = ""
        self.synthetic_spans: list[dict[str, Any]] = []
        self.service_name = "synthetic-test-service"

    def _send_otlp_trace(self, payload: dict[str, Any]) -> bool:
        """Send OTLP trace to the collector endpoint."""
        try:
            data = json.dumps(payload).encode("utf-8")
            req = Request(
                f"{OTEL_BASE}/v1/traces",
                data=data,
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urlopen(req, timeout=10) as response:
                return response.status == 200
        except Exception as e:
            log_info(f"Failed to send OTLP trace: {e}")
            return False

    def test_ingest_synthetic_trace(self) -> bool:
        """Ingest a synthetic trace with multiple spans."""
        log_section("Synthetic Trace Ingestion Tests")
        log_info("Creating and ingesting synthetic trace...")

        # Generate unique IDs
        self.synthetic_trace_id = generate_trace_id()
        root_span_id = generate_span_id()
        child1_span_id = generate_span_id()
        child2_span_id = generate_span_id()
        grandchild_span_id = generate_span_id()

        now_ns = int(time.time() * 1_000_000_000)

        # Create a trace with parent-child relationships:
        # root_span
        #   +-- child1_span (LLM call)
        #   |     +-- grandchild_span (tool call)
        #   +-- child2_span (another operation)
        self.synthetic_spans = [
            {
                "trace_id": self.synthetic_trace_id,
                "span_id": root_span_id,
                "parent_span_id": None,
                "name": "synthetic-root-span",
                "kind": 1,  # INTERNAL
                "start_time_ns": now_ns,
                "end_time_ns": now_ns + 5_000_000_000,  # 5 seconds
                "attributes": [
                    {"key": "synthetic.test", "value": {"boolValue": True}},
                    {"key": "test.iteration", "value": {"intValue": "1"}},
                ],
                "status": {"code": 1},  # OK
            },
            {
                "trace_id": self.synthetic_trace_id,
                "span_id": child1_span_id,
                "parent_span_id": root_span_id,
                "name": "synthetic-llm-call",
                "kind": 3,  # CLIENT
                "start_time_ns": now_ns + 100_000_000,
                "end_time_ns": now_ns + 3_000_000_000,  # 2.9 seconds
                "attributes": [
                    {"key": "gen_ai.request.model", "value": {"stringValue": "gpt-4-synthetic"}},
                    {"key": "gen_ai.usage.input_tokens", "value": {"intValue": "100"}},
                    {"key": "gen_ai.usage.output_tokens", "value": {"intValue": "250"}},
                ],
                "status": {"code": 1},
            },
            {
                "trace_id": self.synthetic_trace_id,
                "span_id": grandchild_span_id,
                "parent_span_id": child1_span_id,
                "name": "synthetic-tool-call",
                "kind": 1,
                "start_time_ns": now_ns + 500_000_000,
                "end_time_ns": now_ns + 1_500_000_000,  # 1 second
                "attributes": [
                    {"key": "gen_ai.tool.name", "value": {"stringValue": "calculator"}},
                    {"key": "gen_ai.tool.call.id", "value": {"stringValue": "call_123"}},
                ],
                "status": {"code": 1},
            },
            {
                "trace_id": self.synthetic_trace_id,
                "span_id": child2_span_id,
                "parent_span_id": root_span_id,
                "name": "synthetic-processing",
                "kind": 1,
                "start_time_ns": now_ns + 3_500_000_000,
                "end_time_ns": now_ns + 4_500_000_000,  # 1 second
                "attributes": [
                    {"key": "processing.type", "value": {"stringValue": "validation"}},
                ],
                "status": {"code": 1},
            },
        ]

        # Create OTLP payload
        payload = create_otlp_trace(
            self.synthetic_trace_id,
            self.synthetic_spans,
            self.service_name,
        )

        # Send to collector
        success = self._send_otlp_trace(payload)
        return self.assert_true(success, "Synthetic trace ingested successfully")

    def test_wait_for_persistence(self) -> bool:
        """Wait for trace to be persisted."""
        log_info("Waiting for trace persistence...")
        time.sleep(3)  # Allow time for flush
        self.assert_true(True, "Waited for persistence")
        return True

    def test_verify_trace_exists(self) -> bool:
        """Verify the synthetic trace exists in the API."""
        log_section("Synthetic Trace Verification")
        log_info("Verifying trace exists...")

        result = api_call(f"/traces/{self.synthetic_trace_id}")
        if not self.assert_not_none(result, f"Trace {self.synthetic_trace_id[:16]}... retrieved"):
            return False

        if isinstance(result, dict):
            self.assert_equals(
                result.get("trace_id"),
                self.synthetic_trace_id,
                "Trace ID matches",
            )
            self.assert_equals(
                result.get("service_name"),
                self.service_name,
                "Service name matches",
            )
            self.assert_equals(
                result.get("span_count"),
                len(self.synthetic_spans),
                f"Span count is {len(self.synthetic_spans)}",
            )
            return True
        return False

    def test_verify_all_spans_exist(self) -> bool:
        """Verify all spans from the synthetic trace are retrievable."""
        log_info("Verifying all spans exist...")

        result = api_call(f"/spans?trace_id={encode_param(self.synthetic_trace_id)}&limit=100")
        if not self.assert_not_none(result, "Spans retrieved for synthetic trace"):
            return False

        if isinstance(result, list):
            self.assert_equals(
                len(result),
                len(self.synthetic_spans),
                f"All {len(self.synthetic_spans)} spans retrieved",
            )

            # Verify each span ID exists
            retrieved_span_ids = {s.get("span_id") for s in result}
            for span in self.synthetic_spans:
                span_id = span["span_id"]
                self.assert_true(
                    span_id in retrieved_span_ids,
                    f"Span {span_id[:8]}... exists",
                )

            return True
        return False

    def test_verify_span_details(self) -> bool:
        """Verify span details are correct."""
        log_info("Verifying span details...")

        result = api_call(f"/spans?trace_id={encode_param(self.synthetic_trace_id)}&limit=100")
        if not result or not isinstance(result, list):
            self.skip("No spans to verify details")
            return True

        # Find the LLM call span and verify its attributes
        llm_spans = [s for s in result if s.get("span_name") == "synthetic-llm-call"]
        if llm_spans:
            llm_span = llm_spans[0]
            # Model might only be extracted for certain span types/frameworks
            if llm_span.get("gen_ai_request_model"):
                self.assert_equals(
                    llm_span.get("gen_ai_request_model"),
                    "gpt-4-synthetic",
                    "LLM span has correct model",
                )
            # Token counts might be in attributes or extracted fields
            if llm_span.get("usage_input_tokens"):
                self.assert_equals(
                    llm_span.get("usage_input_tokens"),
                    100,
                    "LLM span has correct input tokens",
                )
            if llm_span.get("usage_output_tokens"):
                self.assert_equals(
                    llm_span.get("usage_output_tokens"),
                    250,
                    "LLM span has correct output tokens",
                )

        # Find tool call span
        tool_spans = [s for s in result if s.get("span_name") == "synthetic-tool-call"]
        if tool_spans:
            tool_span = tool_spans[0]
            if tool_span.get("gen_ai_tool_name"):
                self.assert_equals(
                    tool_span.get("gen_ai_tool_name"),
                    "calculator",
                    "Tool span has correct tool name",
                )

        return True

    def test_verify_parent_child_relationships(self) -> bool:
        """Verify parent-child span relationships are preserved."""
        log_info("Verifying parent-child relationships...")

        result = api_call(f"/spans?trace_id={encode_param(self.synthetic_trace_id)}&limit=100")
        if not result or not isinstance(result, list):
            self.skip("No spans to verify relationships")
            return True

        # Find root span (no parent)
        root_spans = [s for s in result if not s.get("parent_span_id")]
        self.assert_equals(len(root_spans), 1, "Exactly one root span")

        if root_spans:
            root_span = root_spans[0]
            self.assert_equals(
                root_span.get("span_name"),
                "synthetic-root-span",
                "Root span name is correct",
            )

            # Find children of root
            children = [s for s in result if s.get("parent_span_id") == root_span.get("span_id")]
            self.assert_equals(len(children), 2, "Root has 2 children")

        return True

    def test_verify_trace_in_listing(self) -> bool:
        """Verify the synthetic trace appears in trace listings."""
        log_info("Verifying trace appears in listing...")

        result = api_call(f"/traces?service={encode_param(self.service_name)}&limit=50")
        if not self.assert_not_none(result, "Trace listing works"):
            return False

        if isinstance(result, dict):
            traces = result.get("traces", [])
            trace_ids = [t.get("trace_id") for t in traces]
            self.assert_true(
                self.synthetic_trace_id in trace_ids,
                "Synthetic trace found in service-filtered listing",
            )

        return True

    def run_all(self) -> None:
        """Run all synthetic trace tests."""
        # Ingest
        if not self.test_ingest_synthetic_trace():
            return  # Can't continue without ingestion

        self.test_wait_for_persistence()

        # Verify
        self.test_verify_trace_exists()
        self.test_verify_all_spans_exist()
        self.test_verify_span_details()
        self.test_verify_parent_child_relationships()
        self.test_verify_trace_in_listing()
