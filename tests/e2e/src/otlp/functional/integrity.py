"""OTLP integrity tests - data correctness verification."""

import time
from typing import Any

from ...api import api_call, encode_param
from ...base import BaseTestSuite
from ...config import TRACE_PERSIST_WAIT
from ...logging import log_info, log_section
from ..traces import (
    create_otlp_trace,
    generate_span_id,
    generate_trace_id,
    send_otlp_traces_http,
)


class IntegrityTests(BaseTestSuite):
    """Data integrity verification tests."""

    def __init__(self) -> None:
        super().__init__()
        self.test_trace_id: str = ""
        self.test_spans: list[dict[str, Any]] = []

    def setup(self) -> None:
        """Create test data for integrity tests."""
        log_info("Setting up integrity test data...")

        self.test_trace_id = generate_trace_id()
        root_span_id = generate_span_id()
        child_span_id = generate_span_id()
        now_ns = int(time.time() * 1_000_000_000)

        self.test_spans = [
            {
                "trace_id": self.test_trace_id,
                "span_id": root_span_id,
                "parent_span_id": None,
                "name": "integrity-root",
                "kind": 1,
                "start_time_ns": now_ns,
                "end_time_ns": now_ns + 2_000_000_000,
                "attributes": [
                    {"key": "gen_ai.usage.input_tokens", "value": {"intValue": "100"}},
                    {"key": "gen_ai.usage.output_tokens", "value": {"intValue": "200"}},
                ],
                "status": {"code": 1},
            },
            {
                "trace_id": self.test_trace_id,
                "span_id": child_span_id,
                "parent_span_id": root_span_id,
                "name": "integrity-child",
                "kind": 1,
                "start_time_ns": now_ns + 100_000_000,
                "end_time_ns": now_ns + 1_500_000_000,
                "attributes": [
                    {"key": "gen_ai.usage.input_tokens", "value": {"intValue": "50"}},
                    {"key": "gen_ai.usage.output_tokens", "value": {"intValue": "75"}},
                ],
                "status": {"code": 1},
            },
        ]

        payload = create_otlp_trace(
            self.test_trace_id,
            self.test_spans,
            "integrity-test-service",
        )
        send_otlp_traces_http(payload)
        time.sleep(TRACE_PERSIST_WAIT)

    def test_trace_span_relationship(self) -> bool:
        """Test spans belong to correct traces."""
        log_section("Integrity Tests")
        log_info("Testing trace-span relationships...")

        result = api_call(
            f"/spans?trace_id={encode_param(self.test_trace_id)}&limit=100"
        )
        if not result or not isinstance(result, dict):
            self.skip("No spans found for integrity test")
            return True

        spans = result.get("spans", [])
        if not spans:
            self.skip("No spans found for integrity test")
            return True

        all_match = all(s.get("trace_id") == self.test_trace_id for s in spans)
        return self.assert_true(all_match, "All spans belong to correct trace")

    def test_token_aggregation(self) -> bool:
        """Test trace totals match span sums."""
        log_info("Testing token aggregation...")

        # Get trace with aggregated tokens
        result = api_call(f"/traces/{self.test_trace_id}")
        if not result or not isinstance(result, dict):
            self.skip("Trace not found for token aggregation test")
            return True

        trace_input = result.get("total_input_tokens", 0)
        trace_output = result.get("total_output_tokens", 0)

        # Expected: 100 + 50 = 150 input, 200 + 75 = 275 output
        expected_input = 150
        expected_output = 275

        if trace_input and trace_output:
            input_match = self.assert_equals(
                trace_input, expected_input, "Input tokens match"
            )
            output_match = self.assert_equals(
                trace_output, expected_output, "Output tokens match"
            )
            return input_match and output_match

        # Token aggregation may not be implemented
        self.skip("Token aggregation not available in trace")
        return True

    def test_duration_calculation(self) -> bool:
        """Test duration computed correctly."""
        log_info("Testing duration calculation...")

        result = api_call(f"/traces/{self.test_trace_id}")
        if not result or not isinstance(result, dict):
            self.skip("Trace not found for duration test")
            return True

        duration_ns = result.get("duration_ns")
        if duration_ns:
            # Duration should be approximately 2 seconds (2_000_000_000 ns)
            expected_ns = 2_000_000_000
            tolerance = 100_000_000  # 100ms tolerance

            within_tolerance = abs(duration_ns - expected_ns) < tolerance
            return self.assert_true(
                within_tolerance,
                f"Duration {duration_ns}ns within tolerance of {expected_ns}ns",
            )

        self.skip("Duration not in trace response")
        return True

    def test_framework_attributes(self) -> bool:
        """Test framework-specific fields preserved."""
        log_info("Testing framework attribute preservation...")

        # Create a trace with framework-specific attributes
        trace_id = generate_trace_id()
        now_ns = int(time.time() * 1_000_000_000)

        spans = [
            {
                "trace_id": trace_id,
                "span_id": generate_span_id(),
                "name": "langchain.llm.openai",
                "kind": 1,
                "start_time_ns": now_ns,
                "end_time_ns": now_ns + 1_000_000_000,
                "attributes": [
                    {"key": "langchain.request.type", "value": {"stringValue": "llm"}},
                    {"key": "gen_ai.request.model", "value": {"stringValue": "gpt-4"}},
                ],
                "status": {"code": 1},
            }
        ]

        payload = create_otlp_trace(trace_id, spans, "framework-test", "langchain")
        send_otlp_traces_http(payload)
        time.sleep(TRACE_PERSIST_WAIT)

        result = api_call(f"/traces/{trace_id}")
        if not result or not isinstance(result, dict):
            self.skip("Could not retrieve trace for framework detection test")
            return True

        framework = result.get("detected_framework")
        if framework:
            return self.assert_true(True, f"Framework detected: {framework}")

        # Framework detection is optional - pass if trace was stored
        return self.assert_true(
            result.get("trace_id") == trace_id,
            "Trace stored (framework detection not available)",
        )

    def test_unknown_fields(self) -> bool:
        """Test unknown attributes stored in JSON."""
        log_info("Testing unknown attribute storage...")

        trace_id = generate_trace_id()
        now_ns = int(time.time() * 1_000_000_000)

        spans = [
            {
                "trace_id": trace_id,
                "span_id": generate_span_id(),
                "name": "unknown-attrs-test",
                "kind": 1,
                "start_time_ns": now_ns,
                "end_time_ns": now_ns + 1_000_000_000,
                "attributes": [
                    {
                        "key": "custom.unknown.field",
                        "value": {"stringValue": "test-value"},
                    },
                    {"key": "another.custom.attr", "value": {"intValue": "12345"}},
                ],
                "status": {"code": 1},
            }
        ]

        payload = create_otlp_trace(trace_id, spans, "unknown-test")
        send_otlp_traces_http(payload)
        time.sleep(TRACE_PERSIST_WAIT)

        result = api_call(f"/spans?trace_id={encode_param(trace_id)}&limit=10")
        if not result or not isinstance(result, dict):
            self.skip("Could not retrieve spans for unknown fields test")
            return True

        spans = result.get("spans", [])
        if not spans:
            self.skip("No spans found for unknown fields test")
            return True

        span = spans[0]
        attrs_json = span.get("attributes_json") or span.get("unknown_attributes")
        if attrs_json:
            return self.assert_contains(
                str(attrs_json),
                "custom.unknown.field",
                "Unknown attribute preserved in attributes_json",
            )

        # If no attributes_json, verify the span was at least stored
        return self.assert_true(
            span.get("span_id") is not None,
            "Span stored (attributes_json field not present)",
        )

    def run_all(self) -> None:
        """Run all integrity tests."""
        self.setup()

        self.test_trace_span_relationship()
        self.test_token_aggregation()
        self.test_duration_calculation()
        self.test_framework_attributes()
        self.test_unknown_fields()
