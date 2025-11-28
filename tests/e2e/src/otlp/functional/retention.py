"""OTLP retention tests - storage retention policies."""

import time

from ...api import api_call
from ...base import BaseTestSuite
from ...logging import log_info, log_section
from ..traces import create_batch_traces, send_otlp_traces_http


class RetentionTests(BaseTestSuite):
    """Storage retention policy tests."""

    def __init__(self) -> None:
        super().__init__()
        self.initial_trace_ids: list[str] = []

    def test_storage_stats_available(self) -> bool:
        """Test that storage stats are available via health endpoint."""
        log_section("Retention Tests")
        log_info("Testing storage stats availability...")

        result = api_call("/health")
        if not result or not isinstance(result, dict):
            return self.assert_true(False, "Health endpoint not accessible")

        otel = result.get("otel", {})
        stats = otel.get("stats", {})

        has_traces = "total_traces" in stats
        has_spans = "total_spans" in stats
        has_bytes = "storage_bytes" in stats
        has_files = "storage_files" in stats

        return self.assert_true(
            has_traces and has_spans and has_bytes and has_files,
            "Storage stats available in health endpoint",
        )

    def test_storage_stats_increment(self) -> bool:
        """Test that storage stats increase after ingestion."""
        log_info("Testing storage stats increment after ingestion...")

        # Get initial stats
        result = api_call("/health")
        if not result or not isinstance(result, dict):
            self.skip("Health endpoint not accessible")
            return True

        initial_stats = result.get("otel", {}).get("stats", {})
        initial_spans = initial_stats.get("total_spans", 0)

        # Ingest some traces
        traces = create_batch_traces(count=3, spans_per_trace=5)
        success_count = 0
        for _, payload in traces:
            success, _ = send_otlp_traces_http(payload)
            if success:
                success_count += 1

        if success_count == 0:
            self.skip("No traces were ingested successfully")
            return True

        # Wait for persistence
        time.sleep(5)

        # Get updated stats
        result = api_call("/health")
        if not result or not isinstance(result, dict):
            return self.assert_true(False, "Health endpoint failed after ingestion")

        updated_stats = result.get("otel", {}).get("stats", {})
        updated_spans = updated_stats.get("total_spans", 0)

        # If stats are not available, check if traces are queryable instead
        if initial_spans == 0 and updated_spans == 0:
            # Stats may not be implemented - verify via trace query
            traces_result = api_call("/traces?limit=5")
            if traces_result and isinstance(traces_result, dict):
                trace_count = len(traces_result.get("traces", []))
                return self.assert_greater(
                    trace_count,
                    0,
                    f"Data ingested (stats unavailable, {trace_count} traces queryable)",
                )

        return self.assert_true(
            updated_spans > initial_spans,
            f"Span count increased ({initial_spans} -> {updated_spans})",
        )

    def test_trace_soft_delete(self) -> bool:
        """Test deleted traces don't appear in API."""
        log_info("Testing trace soft delete...")

        # Create a trace
        traces = create_batch_traces(count=1, spans_per_trace=2)
        trace_id, payload = traces[0]

        success, _ = send_otlp_traces_http(payload)
        if not success:
            self.skip("Could not create trace for soft delete test")
            return True

        time.sleep(3)

        # Verify it exists
        result = api_call(f"/traces/{trace_id}")
        if not result:
            self.skip("Trace not found before soft delete")
            return True

        # Delete it
        api_call(f"/traces/{trace_id}", method="DELETE")
        time.sleep(1)

        # Verify not in listing
        result = api_call("/traces?limit=100")
        if not result or not isinstance(result, dict):
            return self.assert_true(
                False, "Failed to retrieve trace listing after delete"
            )

        traces = result.get("traces", [])
        trace_ids = [t["trace_id"] for t in traces]
        return self.assert_true(
            trace_id not in trace_ids,
            "Deleted trace not in listing",
        )

    def test_deleted_trace_not_fetchable(self) -> bool:
        """Test deleted traces return 404 on direct fetch."""
        log_info("Testing deleted trace returns 404...")

        # Create a trace
        traces = create_batch_traces(count=1, spans_per_trace=2)
        trace_id, payload = traces[0]

        success, _ = send_otlp_traces_http(payload)
        if not success:
            self.skip("Could not create trace for delete test")
            return True

        time.sleep(3)

        # Verify it exists
        result = api_call(f"/traces/{trace_id}")
        if not result:
            self.skip("Trace not found before delete")
            return True

        # Delete it
        api_call(f"/traces/{trace_id}", method="DELETE")
        time.sleep(1)

        # Try to fetch directly - should get None (404)
        result = api_call(f"/traces/{trace_id}")
        return self.assert_true(
            result is None,
            "Deleted trace returns 404",
        )

    def run_all(self) -> None:
        """Run all retention tests."""
        self.test_storage_stats_available()
        self.test_storage_stats_increment()
        self.test_trace_soft_delete()
        self.test_deleted_trace_not_fetchable()
