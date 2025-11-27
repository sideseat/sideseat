"""Data integrity tests."""

import time

from ..api import api_call, encode_param
from ..logging import log_info, log_section, log_success, log_warn
from .base import BaseTestSuite


class IntegrityTests(BaseTestSuite):
    """Data integrity test suite."""

    def test_framework_detection_strands(self) -> bool:
        """Test that Strands framework is detected."""
        log_section("Data Integrity Tests")
        log_info("Testing Strands framework detection...")

        strands_found = any(
            "strands" in t.get("detected_framework", "").lower() for t in self.traces
        )
        self.assert_true(strands_found, "Strands framework detected in traces")
        return True

    def test_token_usage_present(self) -> bool:
        """Test that token usage data is captured."""
        log_info("Testing token usage data...")

        spans_with_tokens = 0
        total_input = 0
        total_output = 0

        for s in self.all_spans:
            input_tokens = s.get("usage_input_tokens") or 0
            output_tokens = s.get("usage_output_tokens") or 0
            if input_tokens > 0 or output_tokens > 0:
                spans_with_tokens += 1
                total_input += input_tokens
                total_output += output_tokens

        self.assert_greater(spans_with_tokens, 0, f"Spans with token data: {spans_with_tokens}")
        self.assert_greater(total_input, 0, f"Total input tokens captured: {total_input}")
        self.assert_greater(total_output, 0, f"Total output tokens captured: {total_output}")

        return True

    def test_timestamps_valid(self) -> bool:
        """Test that timestamps are valid."""
        log_info("Testing timestamp validity...")

        # Current time in nanoseconds (roughly)
        now_ns = int(time.time() * 1_000_000_000)
        # Allow timestamps from 1 hour ago to now + 1 minute
        min_valid = now_ns - (60 * 60 * 1_000_000_000)  # 1 hour ago
        max_valid = now_ns + (60 * 1_000_000_000)  # 1 minute future

        valid_count = 0
        invalid_count = 0

        for s in self.spans[:10]:  # Check first 10
            start = s.get("start_time_ns", 0)
            if min_valid <= start <= max_valid:
                valid_count += 1
            else:
                invalid_count += 1

        self.assert_greater(valid_count, 0, f"Timestamps are in valid range: {valid_count} valid")
        if invalid_count > 0:
            log_warn(f"{invalid_count} spans have timestamps outside expected range")

        return True

    def test_duration_calculated(self) -> bool:
        """Test that duration is calculated correctly."""
        log_info("Testing duration calculation...")

        spans_with_duration = 0
        correct_duration = 0

        for s in self.spans[:20]:  # Check first 20
            start = s.get("start_time_ns", 0)
            end = s.get("end_time_ns")
            duration = s.get("duration_ns")

            if end and duration:
                spans_with_duration += 1
                expected = end - start
                if duration == expected:
                    correct_duration += 1

        self.assert_greater(spans_with_duration, 0, f"Spans with duration: {spans_with_duration}")

        if spans_with_duration > 0:
            self.assert_equals(
                correct_duration,
                spans_with_duration,
                "Duration correctly calculated (end - start)",
            )

        return True

    def test_span_hierarchy(self) -> bool:
        """Test span parent-child relationships."""
        log_info("Testing span hierarchy...")

        if not self.traces:
            self.skip("No traces for hierarchy test")
            return True

        # Get all spans for first trace
        trace_id = self.traces[0]["trace_id"]
        result = api_call(f"/spans?trace_id={encode_param(trace_id)}&limit=500")
        if not result or not isinstance(result, list):
            self.skip("Could not get spans for hierarchy test")
            return True

        spans = result
        span_ids = {s["span_id"] for s in spans}

        # Count root spans (no parent) and child spans
        root_spans = 0
        child_spans = 0
        orphan_spans = 0

        for s in spans:
            parent = s.get("parent_span_id")
            if not parent:
                root_spans += 1
            elif parent in span_ids:
                child_spans += 1
            else:
                orphan_spans += 1  # Parent not in this trace

        self.assert_greater(root_spans, 0, f"Trace has root span(s): {root_spans}")
        log_success(f"Child spans: {child_spans}, Orphans: {orphan_spans}")

        return True

    def test_trace_span_count_consistency(self) -> bool:
        """Test that trace span_count matches actual spans."""
        log_info("Testing trace-span count consistency...")

        if not self.traces:
            self.skip("No traces for consistency test")
            return True

        all_consistent = True

        # Check first 3 traces
        for trace in self.traces[:3]:
            trace_id = trace["trace_id"]
            expected_count = trace.get("span_count", 0)

            result = api_call(f"/spans?trace_id={encode_param(trace_id)}&limit=500")
            if result and isinstance(result, list):
                actual_count = len(result)
                if actual_count == expected_count:
                    log_success(f"Trace {trace_id[:8]}... span count matches: {actual_count}")
                else:
                    self.assert_true(
                        False,
                        f"Trace {trace_id[:8]}... count mismatch: expected {expected_count}, got {actual_count}",
                    )
                    all_consistent = False

        if all_consistent:
            self.assert_true(True, "All trace span counts are consistent")

        return True

    def test_genai_fields_extracted(self) -> bool:
        """Test that GenAI-specific fields are extracted."""
        log_info("Testing GenAI field extraction...")

        genai_data: dict[str, set[str]] = {
            "models": set(),
            "agents": set(),
            "tools": set(),
            "categories": set(),
        }

        for s in self.all_spans:
            if m := s.get("gen_ai_request_model"):
                genai_data["models"].add(m)
            if a := s.get("gen_ai_agent_name"):
                genai_data["agents"].add(a)
            if t := s.get("gen_ai_tool_name"):
                genai_data["tools"].add(t)
            if c := s.get("detected_category"):
                genai_data["categories"].add(c)

        self.assert_greater(len(genai_data["models"]), 0, f"Models detected: {genai_data['models']}")
        self.assert_greater(
            len(genai_data["categories"]),
            0,
            f"Categories detected: {genai_data['categories']}",
        )

        if genai_data["agents"]:
            log_success(f"Agents detected: {genai_data['agents']}")
        if genai_data["tools"]:
            log_success(f"Tools detected: {genai_data['tools']}")

        return True

    def run_all(self) -> None:
        """Run all integrity tests."""
        self.test_framework_detection_strands()
        self.test_token_usage_present()
        self.test_timestamps_valid()
        self.test_duration_calculated()
        self.test_span_hierarchy()
        self.test_trace_span_count_consistency()
        self.test_genai_fields_extracted()
