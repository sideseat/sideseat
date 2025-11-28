"""OTLP API tests - traces, spans, and filters endpoints."""

import time

from ...api import api_call, encode_param
from ...base import BaseTestSuite
from ...config import TRACE_PERSIST_WAIT
from ...logging import log_info, log_section
from ..traces import create_simple_trace, send_otlp_traces_http


class APITests(BaseTestSuite):
    """API endpoint tests for traces and spans."""

    def __init__(self) -> None:
        super().__init__()
        self.test_trace_id: str = ""
        self.test_service: str = "api-test-service"

    def setup(self) -> None:
        """Create test data before running API tests."""
        log_info("Setting up test data...")

        # Create a trace for testing
        trace_id, payload = create_simple_trace(
            service_name=self.test_service,
            span_count=5,
            with_genai=True,
        )

        success, _ = send_otlp_traces_http(payload)
        if success:
            self.test_trace_id = trace_id
            time.sleep(TRACE_PERSIST_WAIT)

    def test_trace_list(self) -> bool:
        """Test GET /api/v1/traces."""
        log_section("API Tests")
        log_info("Testing trace listing...")

        result = api_call("/traces")
        if not self.assert_not_none(result, "Trace listing returns data"):
            return False

        if isinstance(result, dict):
            traces = result.get("traces", [])
            self.traces = traces
            self.assert_greater(len(traces), 0, "At least one trace exists")
            self.assert_true("has_more" in result, "Response has 'has_more'")
            self.assert_true("next_cursor" in result, "Response has 'next_cursor'")
            return True

        return False

    def test_trace_filters(self) -> bool:
        """Test filtering by service, framework, attributes."""
        log_info("Testing trace filters...")

        assertions_made = False

        # Filter by service
        result = api_call(f"/traces?service={encode_param(self.test_service)}")
        if result and isinstance(result, dict):
            traces = result.get("traces", [])
            if traces:
                all_match = all(
                    t.get("service_name") == self.test_service for t in traces
                )
                self.assert_true(all_match, "Service filter works correctly")
                assertions_made = True

        # Filter errors only
        result = api_call("/traces?errors_only=true")
        if result and isinstance(result, dict):
            traces = result.get("traces", [])
            for t in traces:
                if not t.get("has_errors", False):
                    return self.assert_true(
                        False, "errors_only returned non-error trace"
                    )
            if traces:
                assertions_made = True
                self.assert_true(True, "errors_only filter returns only error traces")

        if not assertions_made:
            self.skip("No traces available to test filters")

        return True

    def test_trace_detail(self) -> bool:
        """Test GET /api/v1/traces/{id}."""
        log_info("Testing single trace retrieval...")

        if not self.test_trace_id:
            self.skip("No test trace available")
            return True

        result = api_call(f"/traces/{self.test_trace_id}")
        if not self.assert_not_none(
            result, f"Trace {self.test_trace_id[:16]}... retrieved"
        ):
            return False

        if isinstance(result, dict):
            return self.assert_equals(
                result.get("trace_id"),
                self.test_trace_id,
                "Correct trace returned",
            )

        return False

    def test_trace_delete(self) -> bool:
        """Test DELETE /api/v1/traces/{id}."""
        log_info("Testing trace deletion...")

        # Create a trace specifically for deletion
        trace_id, payload = create_simple_trace(
            service_name="delete-test-service",
            span_count=1,
        )

        success, _ = send_otlp_traces_http(payload)
        if not success:
            self.skip("Could not create trace for deletion test")
            return True

        time.sleep(3)

        # Verify it exists
        result = api_call(f"/traces/{trace_id}")
        if not result:
            self.skip("Trace not found before deletion")
            return True

        # Delete it
        result = api_call(f"/traces/{trace_id}", method="DELETE")
        self.assert_true(result is not None, f"Delete request for {trace_id[:16]}...")

        # Verify it's gone
        time.sleep(1)
        result = api_call(f"/traces/{trace_id}", expect_error=True)
        if result and isinstance(result, dict):
            return self.assert_true(
                result.get("code") == 404 or result.get("error"),
                "Deleted trace returns 404",
            )

        # If result is None, api_call got an error (likely 404)
        return self.assert_true(result is None, "Deleted trace no longer accessible")

    def test_span_list(self) -> bool:
        """Test GET /api/v1/spans."""
        log_info("Testing span listing...")

        result = api_call("/spans?limit=50")
        if not self.assert_not_none(result, "Span listing returns data"):
            return False

        if isinstance(result, dict):
            spans = result.get("spans", [])
            self.spans = spans
            self.assert_true("has_more" in result, "Response has 'has_more'")
            self.assert_true("next_cursor" in result, "Response has 'next_cursor'")
            return self.assert_greater(len(spans), 0, "At least one span exists")

        return False

    def test_span_filters(self) -> bool:
        """Test filtering spans by trace, attributes."""
        log_info("Testing span filters...")

        if not self.test_trace_id:
            self.skip("No test trace available")
            return True

        result = api_call(
            f"/spans?trace_id={encode_param(self.test_trace_id)}&limit=100"
        )
        if not self.assert_not_none(result, "Span filter by trace_id works"):
            return False

        if isinstance(result, dict):
            spans = result.get("spans", [])
            all_match = all(s.get("trace_id") == self.test_trace_id for s in spans)
            return self.assert_true(all_match, "All spans belong to correct trace")

        return False

    def test_pagination(self) -> bool:
        """Test cursor-based pagination."""
        log_info("Testing pagination...")

        all_trace_ids: list[str] = []
        cursor = None
        page = 0
        max_pages = 5

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

        # Check for no duplicates
        unique_ids = set(all_trace_ids)
        return self.assert_equals(
            len(all_trace_ids),
            len(unique_ids),
            "No duplicate traces in pagination",
        )

    def test_filter_options(self) -> bool:
        """Test GET /api/v1/traces/filters."""
        log_info("Testing filter options endpoint...")

        result = api_call("/traces/filters")
        if not self.assert_not_none(result, "Filter options returns data"):
            return False

        if not isinstance(result, dict):
            return self.assert_true(False, "Filter options response is not a dict")

        # Check expected filter option keys
        expected_keys = ["services", "frameworks"]
        found_keys = [key for key in expected_keys if key in result]

        return self.assert_greater(
            len(found_keys),
            0,
            f"Filter options has keys: {found_keys}",
        )

    def run_all(self) -> None:
        """Run all API tests."""
        self.setup()

        self.test_trace_list()
        self.test_trace_filters()
        self.test_trace_detail()
        self.test_span_list()
        self.test_span_filters()
        self.test_pagination()
        self.test_filter_options()
        self.test_trace_delete()
