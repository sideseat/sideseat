"""Trace listing and filtering tests."""

from ..api import api_call, encode_param
from ..logging import log_info, log_section, log_success
from .base import BaseTestSuite


class TraceTests(BaseTestSuite):
    """Trace API test suite."""

    def test_trace_listing_basic(self) -> bool:
        """Test basic trace listing."""
        log_section("Trace Listing Tests")
        log_info("Testing trace listing...")

        result = api_call("/traces")
        if not self.assert_not_none(result, "Trace listing returns data"):
            return False

        if isinstance(result, dict):
            traces = result.get("traces", [])
            self.traces = traces

            self.assert_greater(len(traces), 0, "At least one trace exists")
            self.assert_true("has_more" in result, "Response has 'has_more' field")
            self.assert_true("next_cursor" in result, "Response has 'next_cursor' field")

            return len(traces) > 0
        return False

    def test_trace_structure(self) -> bool:
        """Validate trace data structure."""
        log_info("Validating trace structure...")

        if not self.traces:
            self.skip("No traces to validate")
            return True

        trace = self.traces[0]

        # Required fields
        required_fields = [
            "trace_id",
            "service_name",
            "detected_framework",
            "span_count",
            "start_time_ns",
        ]
        for field in required_fields:
            self.assert_true(field in trace, f"Trace has required field: {field}")

        # Optional but expected fields (informational only)
        expected_fields = [
            "root_span_id",
            "end_time_ns",
            "duration_ns",
            "total_input_tokens",
            "total_output_tokens",
            "total_tokens",
            "has_errors",
        ]
        for field in expected_fields:
            if field in trace:
                log_success(f"Trace has optional field: {field}")

        return True

    def test_trace_pagination_forward(self) -> bool:
        """Test forward pagination through traces."""
        log_info("Testing trace pagination (forward)...")

        all_trace_ids: list[str] = []
        cursor = None
        page = 0
        max_pages = 10

        while page < max_pages:
            url = "/traces?limit=2"
            if cursor:
                url += f"&cursor={encode_param(cursor)}"

            result = api_call(url)
            if not result or not isinstance(result, dict):
                break

            traces = result.get("traces", [])
            for t in traces:
                all_trace_ids.append(t["trace_id"])

            page += 1

            if not result.get("has_more"):
                break
            cursor = result.get("next_cursor")
            if not cursor:
                break

        self.assert_greater(len(all_trace_ids), 0, "Pagination collected traces")

        # Check for duplicates (should be none)
        unique_ids = set(all_trace_ids)
        self.assert_equals(
            len(all_trace_ids),
            len(unique_ids),
            "No duplicate traces in pagination",
        )

        return True

    def test_trace_limit_parameter(self) -> bool:
        """Test trace limit parameter works correctly."""
        log_info("Testing trace limit parameter...")

        for limit in [1, 5, 10]:
            result = api_call(f"/traces?limit={limit}")
            if result and isinstance(result, dict):
                traces = result.get("traces", [])
                self.assert_true(
                    len(traces) <= limit,
                    f"Limit {limit} respected: got {len(traces)} traces",
                )

        return True

    def test_trace_filter_by_service(self) -> bool:
        """Test filtering traces by service name."""
        log_section("Trace Filtering Tests")
        log_info("Testing filter by service...")

        if not self.traces:
            self.skip("No traces for service filter test")
            return True

        service_name = self.traces[0].get("service_name", "")
        if not service_name:
            self.skip("No service name in trace")
            return True

        result = api_call(f"/traces?service={encode_param(service_name)}")
        if not self.assert_not_none(result, f"Filter by service '{service_name}'"):
            return False

        if isinstance(result, dict):
            traces = result.get("traces", [])
            self.assert_greater(len(traces), 0, "Filter returns traces")

            # Verify all returned traces have correct service
            all_match = all(t.get("service_name") == service_name for t in traces)
            self.assert_true(all_match, "All filtered traces have correct service")

        return True

    def test_trace_filter_by_framework(self) -> bool:
        """Test filtering traces by framework."""
        log_info("Testing filter by framework...")

        if not self.traces:
            self.skip("No traces for framework filter test")
            return True

        # Find a trace with detected framework
        framework = None
        for t in self.traces:
            fw = t.get("detected_framework", "")
            if fw and fw != "unknown":
                framework = fw
                break

        if not framework:
            self.skip("No detected framework found")
            return True

        result = api_call(f"/traces?framework={encode_param(framework)}")
        if not self.assert_not_none(result, f"Filter by framework '{framework}'"):
            return False

        if isinstance(result, dict):
            traces = result.get("traces", [])
            self.assert_greater(len(traces), 0, f"Filter returns traces for '{framework}'")

        return True

    def test_trace_filter_errors_only(self) -> bool:
        """Test filtering traces with errors only."""
        log_info("Testing errors_only filter...")

        result = api_call("/traces?errors_only=true")
        if not self.assert_not_none(result, "errors_only filter works"):
            return False

        if isinstance(result, dict):
            traces = result.get("traces", [])
            # All returned traces should have errors (if any)
            for t in traces:
                if not t.get("has_errors", False):
                    self.assert_true(False, "errors_only returned trace without errors")
                    return False

        self.assert_true(True, "errors_only filter correct (or no error traces)")
        return True

    def test_trace_filter_combined(self) -> bool:
        """Test combined trace filters."""
        log_info("Testing combined filters...")

        if not self.traces:
            self.skip("No traces for combined filter test")
            return True

        service = self.traces[0].get("service_name", "")
        framework = self.traces[0].get("detected_framework", "")

        if not service:
            self.skip("No service for combined filter")
            return True

        url = f"/traces?service={encode_param(service)}&framework={encode_param(framework)}"
        result = api_call(url)
        if not self.assert_not_none(result, "Combined filter works"):
            return False

        if isinstance(result, dict):
            traces = result.get("traces", [])
            # Results should match both criteria
            for t in traces:
                match_service = t.get("service_name") == service
                match_framework = t.get("detected_framework") == framework
                if not (match_service and match_framework):
                    self.assert_true(False, "Combined filter returned non-matching trace")
                    return False

        self.assert_true(True, "Combined filters work correctly")
        return True

    def test_single_trace_retrieval(self) -> bool:
        """Test single trace retrieval."""
        log_section("Single Trace Tests")
        log_info("Testing single trace retrieval...")

        if not self.traces:
            self.skip("No traces for single trace test")
            return True

        trace_id = self.traces[0]["trace_id"]
        result = api_call(f"/traces/{trace_id}")

        if not self.assert_not_none(result, f"Single trace retrieved: {trace_id[:16]}..."):
            return False

        if isinstance(result, dict):
            self.assert_equals(result.get("trace_id"), trace_id, "Correct trace returned")
        return True

    def test_trace_not_found(self) -> bool:
        """Test 404 for non-existent trace."""
        log_info("Testing trace not found (404)...")

        result = api_call("/traces/00000000000000000000000000000000", expect_error=True)
        self.assert_true(
            result is not None
            and isinstance(result, dict)
            and (result.get("error") or result.get("code") == 404),
            "Non-existent trace returns 404",
        )
        return True

    def run_all(self) -> None:
        """Run all trace tests."""
        # Listing tests
        self.test_trace_listing_basic()
        self.test_trace_structure()
        self.test_trace_pagination_forward()
        self.test_trace_limit_parameter()

        # Filtering tests
        self.test_trace_filter_by_service()
        self.test_trace_filter_by_framework()
        self.test_trace_filter_errors_only()
        self.test_trace_filter_combined()

        # Single trace tests
        self.test_single_trace_retrieval()
        self.test_trace_not_found()
