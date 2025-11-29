"""OTLP load tests - throughput and concurrency benchmarks."""

import concurrent.futures
import statistics
import time
from typing import Any

from ...base import BaseTestSuite
from ...logging import log_info, log_section
from ..traces import create_batch_traces, create_simple_trace, send_otlp_traces_http


class LoadTests(BaseTestSuite):
    """Load and throughput tests.

    These tests measure system performance under load conditions.
    Results are logged but tests pass as long as the system remains stable.
    """

    def __init__(self) -> None:
        super().__init__()
        self.results: dict[str, Any] = {}

    def test_ingestion_throughput(self) -> bool:
        """Measure trace ingestion throughput."""
        log_section("Performance Tests - Load")
        log_info("Testing ingestion throughput...")

        batch_count = 100
        traces = create_batch_traces(count=batch_count, spans_per_trace=5)

        start_time = time.time()
        success_count = 0

        for trace_id, payload in traces:
            success, _ = send_otlp_traces_http(payload)
            if success:
                success_count += 1

        elapsed = time.time() - start_time
        throughput = success_count / elapsed if elapsed > 0 else 0

        self.results["ingestion_throughput"] = {
            "traces_sent": success_count,
            "duration_sec": round(elapsed, 2),
            "traces_per_sec": round(throughput, 2),
        }

        log_info(
            f"Throughput: {throughput:.2f} traces/sec ({success_count}/{batch_count})"
        )

        return self.assert_greater(
            success_count,
            int(batch_count * 0.9),  # 90% success rate minimum
            f"Ingestion throughput: {throughput:.2f} traces/sec",
        )

    def test_concurrent_ingestion(self) -> bool:
        """Test concurrent trace ingestion with multiple threads."""
        log_info("Testing concurrent ingestion...")

        concurrent_workers = 10
        traces_per_worker = 20
        total_traces = concurrent_workers * traces_per_worker

        all_traces = create_batch_traces(count=total_traces, spans_per_trace=3)

        # Split into chunks for workers
        chunks = [
            all_traces[i : i + traces_per_worker]
            for i in range(0, len(all_traces), traces_per_worker)
        ]

        def send_chunk(chunk: list) -> int:
            success = 0
            for trace_id, payload in chunk:
                ok, _ = send_otlp_traces_http(payload)
                if ok:
                    success += 1
            return success

        start_time = time.time()

        with concurrent.futures.ThreadPoolExecutor(
            max_workers=concurrent_workers
        ) as executor:
            results = list(executor.map(send_chunk, chunks))

        elapsed = time.time() - start_time
        total_success = sum(results)
        throughput = total_success / elapsed if elapsed > 0 else 0

        self.results["concurrent_ingestion"] = {
            "workers": concurrent_workers,
            "total_traces": total_success,
            "duration_sec": round(elapsed, 2),
            "traces_per_sec": round(throughput, 2),
        }

        log_info(
            f"Concurrent throughput: {throughput:.2f} traces/sec ({total_success}/{total_traces})"
        )

        return self.assert_greater(
            total_success,
            int(total_traces * 0.85),  # 85% success rate for concurrent
            f"Concurrent ingestion: {throughput:.2f} traces/sec",
        )

    def test_large_batch(self) -> bool:
        """Test ingestion of large batches."""
        log_info("Testing large batch ingestion...")

        # Create a trace with many spans
        trace_id, payload = create_simple_trace(
            service_name="large-batch-test",
            span_count=50,
            with_genai=True,
        )

        start_time = time.time()
        success, status = send_otlp_traces_http(payload)
        elapsed = time.time() - start_time

        self.results["large_batch"] = {
            "span_count": 50,
            "success": success,
            "duration_sec": round(elapsed, 3),
        }

        log_info(f"Large batch (50 spans): {elapsed:.3f}s")

        return self.assert_true(success, f"Large batch ingested in {elapsed:.3f}s")

    def test_sustained_load(self) -> bool:
        """Test sustained load over time."""
        log_info("Testing sustained load (30 seconds)...")

        duration_sec = 30
        batch_size = 10
        interval_sec = 1

        start_time = time.time()
        success_count = 0
        total_sent = 0
        latencies: list[float] = []

        while time.time() - start_time < duration_sec:
            batch_start = time.time()
            traces = create_batch_traces(count=batch_size, spans_per_trace=2)

            for trace_id, payload in traces:
                send_start = time.time()
                success, _ = send_otlp_traces_http(payload)
                latencies.append(time.time() - send_start)
                total_sent += 1
                if success:
                    success_count += 1

            # Wait for next interval
            elapsed_batch = time.time() - batch_start
            if elapsed_batch < interval_sec:
                time.sleep(interval_sec - elapsed_batch)

        total_elapsed = time.time() - start_time
        throughput = success_count / total_elapsed if total_elapsed > 0 else 0

        avg_latency = statistics.mean(latencies) if latencies else 0
        p95_latency = (
            sorted(latencies)[int(len(latencies) * 0.95)]
            if len(latencies) > 20
            else max(latencies) if latencies else 0
        )

        self.results["sustained_load"] = {
            "duration_sec": round(total_elapsed, 2),
            "total_sent": total_sent,
            "success_count": success_count,
            "throughput": round(throughput, 2),
            "avg_latency_ms": round(avg_latency * 1000, 2),
            "p95_latency_ms": round(p95_latency * 1000, 2),
        }

        log_info(
            f"Sustained load: {throughput:.2f} traces/sec, "
            f"avg latency: {avg_latency*1000:.2f}ms, "
            f"p95: {p95_latency*1000:.2f}ms"
        )

        success_rate = success_count / total_sent if total_sent > 0 else 0
        return self.assert_greater(
            int(success_rate * 100),
            80,  # 80% success rate under sustained load
            f"Sustained load: {success_rate*100:.1f}% success rate",
        )

    def test_burst_load(self) -> bool:
        """Test burst traffic handling."""
        log_info("Testing burst load...")

        burst_size = 50
        traces = create_batch_traces(count=burst_size, spans_per_trace=3)

        # Send all at once (as fast as possible)
        start_time = time.time()
        success_count = 0

        with concurrent.futures.ThreadPoolExecutor(max_workers=20) as executor:
            futures = [
                executor.submit(send_otlp_traces_http, payload) for _, payload in traces
            ]
            for future in concurrent.futures.as_completed(futures):
                success, _ = future.result()
                if success:
                    success_count += 1

        elapsed = time.time() - start_time
        throughput = success_count / elapsed if elapsed > 0 else 0

        self.results["burst_load"] = {
            "burst_size": burst_size,
            "success_count": success_count,
            "duration_sec": round(elapsed, 3),
            "traces_per_sec": round(throughput, 2),
        }

        log_info(
            f"Burst load: {throughput:.2f} traces/sec ({success_count}/{burst_size})"
        )

        return self.assert_greater(
            success_count,
            int(burst_size * 0.8),  # 80% success for burst
            f"Burst load handled: {success_count}/{burst_size}",
        )

    def test_storage_persistence(self) -> bool:
        """Test that traces are persisted correctly to SQLite storage.

        Note: SQLite uses WAL mode with page recycling, so file size may not
        grow after initial allocation. We verify data persistence by checking
        trace count rather than file size.
        """
        log_info("Testing storage persistence...")

        from ...api import api_call
        from ...config import DATA_DIR

        db_path = DATA_DIR / "sideseat.db"

        # Ingest a batch of traces with unique service name
        test_service = f"storage-test-{int(time.time())}"
        traces = create_batch_traces(
            count=20, spans_per_trace=10, service_name=test_service
        )
        success_count = 0
        trace_ids = []

        for trace_id, payload in traces:
            success, _ = send_otlp_traces_http(payload)
            if success:
                success_count += 1
                trace_ids.append(trace_id)

        # Wait for data to be written
        time.sleep(2)

        # Verify traces were persisted by querying for them
        persisted_count = 0
        result = api_call(f"/traces?service={test_service}&limit=100")
        if result and isinstance(result, dict):
            persisted_count = len(result.get("traces", []))

        # Check database file exists
        db_size = db_path.stat().st_size if db_path.exists() else 0

        self.results["storage_persistence"] = {
            "traces_ingested": success_count,
            "traces_persisted": persisted_count,
            "db_size_bytes": db_size,
        }

        log_info(
            f"Storage: {persisted_count}/{success_count} traces persisted, "
            f"DB size: {db_size} bytes"
        )

        # Verify database exists and traces were persisted
        if db_path.exists() and db_size > 0:
            return self.assert_greater(
                persisted_count,
                int(success_count * 0.9),  # Allow 10% loss due to timing
                f"Traces persisted: {persisted_count}/{success_count}",
            )

        return self.assert_true(False, "SQLite database not found or empty")

    def get_results(self) -> dict[str, Any]:
        """Get performance test results."""
        return self.results

    def run_all(self) -> None:
        """Run all load tests."""
        self.test_ingestion_throughput()
        self.test_concurrent_ingestion()
        self.test_large_batch()
        self.test_sustained_load()
        self.test_burst_load()
        self.test_storage_persistence()

        log_info("Performance results summary:")
        for test_name, result in self.results.items():
            log_info(f"  {test_name}: {result}")
