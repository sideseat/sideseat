"""OTLP query performance tests - API response time benchmarks."""

import concurrent.futures
import statistics
import time
from typing import Any

from ...api import api_call, encode_param
from ...base import BaseTestSuite
from ...config import TRACE_PERSIST_WAIT
from ...logging import log_info, log_section
from ..traces import create_batch_traces, send_otlp_traces_http


class QueryTests(BaseTestSuite):
    """Query performance tests.

    These tests measure API query response times under various conditions.
    """

    def __init__(self) -> None:
        super().__init__()
        self.results: dict[str, Any] = {}
        self.test_trace_ids: list[str] = []

    def setup(self) -> None:
        """Seed test data for query tests."""
        log_info("Seeding test data for query performance tests...")

        # Create test traces with varying characteristics
        traces = create_batch_traces(count=50, spans_per_trace=5)

        for trace_id, payload in traces:
            success, _ = send_otlp_traces_http(payload)
            if success:
                self.test_trace_ids.append(trace_id)

        time.sleep(TRACE_PERSIST_WAIT + 2)  # Extra time for indexing
        log_info(f"Seeded {len(self.test_trace_ids)} traces")

    def _measure_latency(self, endpoint: str, iterations: int = 10) -> dict[str, float]:
        """Measure latency statistics for an endpoint."""
        latencies: list[float] = []

        for _ in range(iterations):
            start = time.time()
            result = api_call(endpoint)
            elapsed = time.time() - start

            if result is not None:
                latencies.append(elapsed)

        if not latencies:
            return {"avg_ms": 0, "p50_ms": 0, "p95_ms": 0, "p99_ms": 0}

        sorted_latencies = sorted(latencies)
        return {
            "avg_ms": round(statistics.mean(latencies) * 1000, 2),
            "p50_ms": round(sorted_latencies[len(sorted_latencies) // 2] * 1000, 2),
            "p95_ms": (
                round(sorted_latencies[int(len(sorted_latencies) * 0.95)] * 1000, 2)
                if len(sorted_latencies) >= 20
                else round(sorted_latencies[-1] * 1000, 2)
            ),
            "p99_ms": (
                round(sorted_latencies[int(len(sorted_latencies) * 0.99)] * 1000, 2)
                if len(sorted_latencies) >= 100
                else round(sorted_latencies[-1] * 1000, 2)
            ),
        }

    def test_trace_list_latency(self) -> bool:
        """Measure trace list endpoint latency."""
        log_section("Performance Tests - Query")
        log_info("Testing trace list latency...")

        stats = self._measure_latency("/traces?limit=50", iterations=20)
        self.results["trace_list"] = stats

        log_info(
            f"Trace list: avg={stats['avg_ms']}ms, p50={stats['p50_ms']}ms, p95={stats['p95_ms']}ms"
        )

        # Trace list should respond within 500ms on average
        return self.assert_less(
            int(stats["avg_ms"]),
            500,
            f"Trace list avg latency: {stats['avg_ms']}ms",
        )

    def test_trace_detail_latency(self) -> bool:
        """Measure trace detail endpoint latency."""
        log_info("Testing trace detail latency...")

        if not self.test_trace_ids:
            self.skip("No test traces available")
            return True

        # Test with a known trace
        trace_id = self.test_trace_ids[0]
        stats = self._measure_latency(f"/traces/{trace_id}", iterations=20)
        self.results["trace_detail"] = stats

        log_info(f"Trace detail: avg={stats['avg_ms']}ms, p50={stats['p50_ms']}ms")

        # Detail endpoint should respond within 200ms on average
        return self.assert_less(
            int(stats["avg_ms"]),
            200,
            f"Trace detail avg latency: {stats['avg_ms']}ms",
        )

    def test_span_list_latency(self) -> bool:
        """Measure span list endpoint latency."""
        log_info("Testing span list latency...")

        if not self.test_trace_ids:
            self.skip("No test traces available")
            return True

        trace_id = self.test_trace_ids[0]
        stats = self._measure_latency(
            f"/spans?trace_id={encode_param(trace_id)}&limit=100",
            iterations=20,
        )
        self.results["span_list"] = stats

        log_info(f"Span list: avg={stats['avg_ms']}ms, p50={stats['p50_ms']}ms")

        return self.assert_less(
            int(stats["avg_ms"]),
            300,
            f"Span list avg latency: {stats['avg_ms']}ms",
        )

    def test_filtered_query_latency(self) -> bool:
        """Measure filtered query latency."""
        log_info("Testing filtered query latency...")

        # Test with service filter
        stats = self._measure_latency(
            "/traces?service=perf-test&limit=20",
            iterations=15,
        )
        self.results["filtered_query"] = stats

        log_info(f"Filtered query: avg={stats['avg_ms']}ms, p50={stats['p50_ms']}ms")

        return self.assert_less(
            int(stats["avg_ms"]),
            500,
            f"Filtered query avg latency: {stats['avg_ms']}ms",
        )

    def test_pagination_latency(self) -> bool:
        """Measure pagination performance across pages."""
        log_info("Testing pagination latency...")

        page_latencies: list[float] = []
        cursor = None

        # Test first 5 pages
        for page in range(5):
            endpoint = "/traces?limit=10"
            if cursor:
                endpoint += f"&cursor={encode_param(cursor)}"

            start = time.time()
            result = api_call(endpoint)
            elapsed = time.time() - start

            if result and isinstance(result, dict):
                page_latencies.append(elapsed)
                cursor = result.get("next_cursor")
                if not cursor:
                    break

        if page_latencies:
            avg_latency = statistics.mean(page_latencies) * 1000
            self.results["pagination"] = {
                "pages_tested": len(page_latencies),
                "avg_ms": round(avg_latency, 2),
            }
            log_info(
                f"Pagination: {len(page_latencies)} pages, avg={avg_latency:.2f}ms"
            )

            return self.assert_less(
                int(avg_latency),
                600,
                f"Pagination avg latency: {avg_latency:.2f}ms",
            )

        self.skip("No pagination data collected")
        return True

    def test_concurrent_queries(self) -> bool:
        """Test query performance under concurrent load."""
        log_info("Testing concurrent query performance...")

        concurrent_requests = 20
        endpoints = ["/traces?limit=10"] * concurrent_requests

        def query_endpoint(endpoint: str) -> tuple[bool, float]:
            start = time.time()
            result = api_call(endpoint)
            elapsed = time.time() - start
            return result is not None, elapsed

        start_time = time.time()

        with concurrent.futures.ThreadPoolExecutor(max_workers=10) as executor:
            results = list(executor.map(query_endpoint, endpoints))

        total_elapsed = time.time() - start_time
        success_count = sum(1 for ok, _ in results if ok)
        latencies = [lat for ok, lat in results if ok]

        if latencies:
            avg_latency = statistics.mean(latencies) * 1000
            self.results["concurrent_queries"] = {
                "concurrent_requests": concurrent_requests,
                "success_count": success_count,
                "total_time_sec": round(total_elapsed, 2),
                "avg_latency_ms": round(avg_latency, 2),
            }
            log_info(
                f"Concurrent queries: {success_count}/{concurrent_requests} success, "
                f"avg={avg_latency:.2f}ms"
            )

            return self.assert_greater(
                success_count,
                int(concurrent_requests * 0.9),
                f"Concurrent queries: {success_count}/{concurrent_requests} success",
            )

        self.skip("No concurrent query latencies collected")
        return True

    def test_large_result_set(self) -> bool:
        """Test query performance with large result sets."""
        log_info("Testing large result set handling...")

        # Request maximum limit
        start = time.time()
        result = api_call("/traces?limit=1000")
        elapsed = time.time() - start

        if result and isinstance(result, dict):
            trace_count = len(result.get("traces", []))
            self.results["large_result"] = {
                "requested_limit": 1000,
                "returned_count": trace_count,
                "latency_ms": round(elapsed * 1000, 2),
            }
            log_info(f"Large result: {trace_count} traces in {elapsed*1000:.2f}ms")

            # Large result should still respond within 2 seconds
            return self.assert_less(
                int(elapsed * 1000),
                2000,
                f"Large result latency: {elapsed*1000:.2f}ms",
            )

        self.skip("No large result set data returned")
        return True

    def get_results(self) -> dict[str, Any]:
        """Get query performance results."""
        return self.results

    def run_all(self) -> None:
        """Run all query performance tests."""
        self.setup()

        self.test_trace_list_latency()
        self.test_trace_detail_latency()
        self.test_span_list_latency()
        self.test_filtered_query_latency()
        self.test_pagination_latency()
        self.test_concurrent_queries()
        self.test_large_result_set()

        log_info("Query performance results summary:")
        for test_name, result in self.results.items():
            log_info(f"  {test_name}: {result}")
