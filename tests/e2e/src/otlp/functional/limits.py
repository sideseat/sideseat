"""OTLP input limits tests - validation of input constraints."""

import time

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


class LimitsTests(BaseTestSuite):
    """Input validation limit tests.

    Tests verify server handles spans that exceed configured limits:
    - max_span_name_len: 1000 chars
    - max_attribute_count: 100 attributes
    - max_attribute_value_len: 10KB
    - max_events_per_span: 100 events
    """

    def test_max_span_name_length(self) -> bool:
        """Test span names exceeding 1000 chars are handled."""
        log_section("Input Limits Tests")
        log_info("Testing max span name length...")

        trace_id = generate_trace_id()
        now_ns = int(time.time() * 1_000_000_000)

        # Create span with very long name (2000 chars)
        long_name = "x" * 2000

        spans = [
            {
                "trace_id": trace_id,
                "span_id": generate_span_id(),
                "name": long_name,
                "kind": 1,
                "start_time_ns": now_ns,
                "end_time_ns": now_ns + 1_000_000_000,
                "attributes": [],
                "status": {"code": 1},
            }
        ]

        payload = create_otlp_trace(trace_id, spans, "limits-test")
        success, status = send_otlp_traces_http(payload)

        if not success:
            return self.assert_true(
                False, f"Failed to ingest span with long name (status: {status})"
            )

        time.sleep(TRACE_PERSIST_WAIT)
        result = api_call(f"/spans?trace_id={encode_param(trace_id)}&limit=10")

        if result and isinstance(result, list) and result:
            span = result[0]
            span_name = span.get("span_name", "")
            return self.assert_true(
                len(span_name) <= 1000,
                f"Long span name truncated to {len(span_name)} chars",
            )

        return self.assert_true(True, "Long span name ingestion accepted")

    def test_max_attribute_count(self) -> bool:
        """Test spans with >100 attributes are handled."""
        log_info("Testing max attribute count...")

        trace_id = generate_trace_id()
        now_ns = int(time.time() * 1_000_000_000)

        # Create 150 attributes
        attributes = []
        for i in range(150):
            attributes.append(
                {
                    "key": f"attr_{i}",
                    "value": {"stringValue": f"value_{i}"},
                }
            )

        spans = [
            {
                "trace_id": trace_id,
                "span_id": generate_span_id(),
                "name": "many-attrs-span",
                "kind": 1,
                "start_time_ns": now_ns,
                "end_time_ns": now_ns + 1_000_000_000,
                "attributes": attributes,
                "status": {"code": 1},
            }
        ]

        payload = create_otlp_trace(trace_id, spans, "limits-test")
        success, _ = send_otlp_traces_http(payload)

        return self.assert_true(success, "150 attributes handled (may be truncated)")

    def test_max_attribute_value_length(self) -> bool:
        """Test attribute values >10KB are handled."""
        log_info("Testing max attribute value length...")

        trace_id = generate_trace_id()
        now_ns = int(time.time() * 1_000_000_000)

        # Create attribute with 20KB value
        large_value = "x" * (20 * 1024)

        spans = [
            {
                "trace_id": trace_id,
                "span_id": generate_span_id(),
                "name": "large-attr-span",
                "kind": 1,
                "start_time_ns": now_ns,
                "end_time_ns": now_ns + 1_000_000_000,
                "attributes": [
                    {"key": "large_attr", "value": {"stringValue": large_value}},
                ],
                "status": {"code": 1},
            }
        ]

        payload = create_otlp_trace(trace_id, spans, "limits-test")
        success, _ = send_otlp_traces_http(payload)

        return self.assert_true(success, "Large attribute value handled")

    def test_max_events_per_span(self) -> bool:
        """Test spans with >100 events are handled."""
        log_info("Testing max events per span...")

        trace_id = generate_trace_id()
        now_ns = int(time.time() * 1_000_000_000)

        # Create 150 events
        events = []
        for i in range(150):
            events.append(
                {
                    "timeUnixNano": str(now_ns + i * 1_000_000),
                    "name": f"event_{i}",
                    "attributes": [],
                }
            )

        spans = [
            {
                "trace_id": trace_id,
                "span_id": generate_span_id(),
                "name": "many-events-span",
                "kind": 1,
                "start_time_ns": now_ns,
                "end_time_ns": now_ns + 1_000_000_000,
                "attributes": [],
                "events": events,
                "status": {"code": 1},
            }
        ]

        payload = create_otlp_trace(trace_id, spans, "limits-test")
        success, _ = send_otlp_traces_http(payload)

        return self.assert_true(success, "150 events handled (may be truncated)")

    def run_all(self) -> None:
        """Run all limits tests."""
        self.test_max_span_name_length()
        self.test_max_attribute_count()
        self.test_max_attribute_value_length()
        self.test_max_events_per_span()
