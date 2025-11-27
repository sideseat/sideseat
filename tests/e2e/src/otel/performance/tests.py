"""Performance tests - large data ingestion and query benchmarking."""

import json
import random
import string
import time
import uuid
from typing import Any
from urllib.request import Request, urlopen

from ...api import api_call, encode_param
from ...config import OTEL_BASE
from ...logging import log_info, log_section, log_success, log_warn
from ..base import BaseTestSuite

# Target ~200MB of trace data
TARGET_SIZE_MB = 200
BATCH_SIZE = 100  # Spans per OTLP request
SPANS_PER_TRACE = 20  # Average spans per trace

# Performance thresholds (in seconds)
QUERY_THRESHOLD_TRACE_LIST = 1.0
QUERY_THRESHOLD_TRACE_SINGLE = 0.5
QUERY_THRESHOLD_SPAN_LIST = 1.0
QUERY_THRESHOLD_SPAN_FILTER = 1.5


def generate_trace_id() -> str:
    """Generate a random 32-character hex trace ID."""
    return uuid.uuid4().hex + uuid.uuid4().hex[:16]


def generate_span_id() -> str:
    """Generate a random 16-character hex span ID."""
    return uuid.uuid4().hex[:16]


def generate_random_string(length: int) -> str:
    """Generate a random string of given length."""
    return "".join(random.choices(string.ascii_letters + string.digits, k=length))


def estimate_span_size(span: dict[str, Any]) -> int:
    """Estimate JSON size of a span in bytes."""
    return len(json.dumps(span))


class PerformanceTests(BaseTestSuite):
    """Performance tests for large data ingestion and query benchmarking."""

    def __init__(self) -> None:
        super().__init__()
        self.trace_ids: list[str] = []
        self.total_spans = 0
        self.total_bytes = 0
        self.ingestion_time = 0.0
        self.service_names = [
            "perf-service-alpha",
            "perf-service-beta",
            "perf-service-gamma",
        ]
        self.models = [
            "gpt-4-perf",
            "claude-3-perf",
            "llama-70b-perf",
        ]
        self.frameworks = [
            "strands-perf",
            "langchain-perf",
            "llamaindex-perf",
        ]

    def _create_span(
        self,
        trace_id: str,
        span_id: str,
        parent_span_id: str | None,
        name: str,
        start_ns: int,
        duration_ns: int,
        service_name: str,
    ) -> dict[str, Any]:
        """Create a span with realistic attributes."""
        attributes = [
            {"key": "perf.test", "value": {"boolValue": True}},
            {"key": "perf.batch_id", "value": {"stringValue": generate_random_string(8)}},
        ]

        # Add GenAI attributes randomly
        if random.random() > 0.5:
            attributes.extend([
                {"key": "gen_ai.request.model", "value": {"stringValue": random.choice(self.models)}},
                {"key": "gen_ai.usage.input_tokens", "value": {"intValue": str(random.randint(100, 10000))}},
                {"key": "gen_ai.usage.output_tokens", "value": {"intValue": str(random.randint(50, 5000))}},
            ])

        # Add some random attributes to increase size
        for i in range(random.randint(3, 10)):
            attributes.append({
                "key": f"perf.attr_{i}",
                "value": {"stringValue": generate_random_string(random.randint(20, 100))},
            })

        span = {
            "traceId": trace_id,
            "spanId": span_id,
            "name": name,
            "kind": random.randint(1, 5),
            "startTimeUnixNano": str(start_ns),
            "endTimeUnixNano": str(start_ns + duration_ns),
            "attributes": attributes,
            "status": {"code": 1 if random.random() > 0.1 else 2},
        }

        if parent_span_id:
            span["parentSpanId"] = parent_span_id

        return span

    def _create_trace_spans(self, trace_id: str, service_name: str, num_spans: int) -> list[dict[str, Any]]:
        """Create a trace with the specified number of spans."""
        spans = []
        now_ns = int(time.time() * 1_000_000_000)

        # Root span
        root_span_id = generate_span_id()
        root_duration = random.randint(1_000_000_000, 10_000_000_000)  # 1-10 seconds
        spans.append(self._create_span(
            trace_id=trace_id,
            span_id=root_span_id,
            parent_span_id=None,
            name=f"perf-root-{generate_random_string(4)}",
            start_ns=now_ns,
            duration_ns=root_duration,
            service_name=service_name,
        ))

        # Child spans
        parent_ids = [root_span_id]
        current_time = now_ns + 10_000_000  # 10ms after root start

        for i in range(num_spans - 1):
            span_id = generate_span_id()
            parent_id = random.choice(parent_ids)
            duration = random.randint(10_000_000, 1_000_000_000)  # 10ms - 1s

            spans.append(self._create_span(
                trace_id=trace_id,
                span_id=span_id,
                parent_span_id=parent_id,
                name=f"perf-span-{i}-{generate_random_string(4)}",
                start_ns=current_time,
                duration_ns=duration,
                service_name=service_name,
            ))

            parent_ids.append(span_id)
            current_time += random.randint(1_000_000, 50_000_000)  # 1-50ms gap

        return spans

    def _create_otlp_payload(self, spans: list[dict[str, Any]], service_name: str) -> dict[str, Any]:
        """Create an OTLP ExportTraceServiceRequest payload."""
        return {
            "resourceSpans": [
                {
                    "resource": {
                        "attributes": [
                            {"key": "service.name", "value": {"stringValue": service_name}},
                            {"key": "service.version", "value": {"stringValue": "1.0.0-perf"}},
                            {"key": "telemetry.sdk.name", "value": {"stringValue": random.choice(self.frameworks)}},
                        ]
                    },
                    "scopeSpans": [
                        {
                            "scope": {"name": "perf-test-scope", "version": "1.0.0"},
                            "spans": spans,
                        }
                    ],
                }
            ]
        }

    def _send_otlp_batch(self, payload: dict[str, Any], max_retries: int = 5) -> bool:
        """Send OTLP trace batch to the collector with retry logic."""
        data = json.dumps(payload).encode("utf-8")

        for attempt in range(max_retries):
            try:
                req = Request(
                    f"{OTEL_BASE}/v1/traces",
                    data=data,
                    headers={"Content-Type": "application/json"},
                    method="POST",
                )
                with urlopen(req, timeout=30) as response:
                    return response.status == 200
            except Exception as e:
                error_str = str(e)
                # Retry on 503 Service Unavailable with exponential backoff
                if "503" in error_str and attempt < max_retries - 1:
                    backoff = 0.5 * (2 ** attempt)  # 0.5s, 1s, 2s, 4s, 8s
                    time.sleep(backoff)
                    continue
                # Log only on final failure
                if attempt == max_retries - 1:
                    log_warn(f"Batch send failed after {max_retries} attempts: {e}")
                return False
        return False

    def _measure_query_time(self, url: str) -> tuple[float, Any]:
        """Measure query execution time and return result."""
        start = time.time()
        result = api_call(url)
        elapsed = time.time() - start
        return elapsed, result

    def test_ingest_large_dataset(self) -> bool:
        """Ingest ~200MB of trace data."""
        log_section("Performance Tests - Data Ingestion")
        log_info(f"Target data size: {TARGET_SIZE_MB}MB")

        target_bytes = TARGET_SIZE_MB * 1024 * 1024
        start_time = time.time()
        batch_count = 0
        failed_batches = 0

        while self.total_bytes < target_bytes:
            # Create a batch of traces
            service_name = random.choice(self.service_names)
            batch_spans = []

            for _ in range(BATCH_SIZE // SPANS_PER_TRACE):
                trace_id = generate_trace_id()
                num_spans = random.randint(SPANS_PER_TRACE - 5, SPANS_PER_TRACE + 10)
                spans = self._create_trace_spans(trace_id, service_name, num_spans)
                batch_spans.extend(spans)
                self.trace_ids.append(trace_id)

            # Create and send OTLP payload
            payload = self._create_otlp_payload(batch_spans, service_name)
            payload_size = len(json.dumps(payload).encode("utf-8"))

            if self._send_otlp_batch(payload):
                self.total_spans += len(batch_spans)
                self.total_bytes += payload_size
                batch_count += 1
            else:
                failed_batches += 1

            # Small delay to prevent overwhelming the server
            time.sleep(0.05)  # 50ms between batches

            # Progress update every 10MB
            if batch_count % 50 == 0:
                progress_mb = self.total_bytes / (1024 * 1024)
                log_info(f"Progress: {progress_mb:.1f}MB / {TARGET_SIZE_MB}MB ({len(self.trace_ids)} traces)")

        self.ingestion_time = time.time() - start_time
        ingested_mb = self.total_bytes / (1024 * 1024)
        throughput = ingested_mb / self.ingestion_time

        log_success(f"Ingested {ingested_mb:.1f}MB in {self.ingestion_time:.1f}s ({throughput:.1f}MB/s)")
        log_success(f"Total traces: {len(self.trace_ids)}, Total spans: {self.total_spans}")

        if failed_batches > 0:
            log_warn(f"Failed batches: {failed_batches}")

        self.assert_greater(int(ingested_mb), TARGET_SIZE_MB - 10, f"Ingested at least {TARGET_SIZE_MB - 10}MB")
        return True

    def test_wait_for_persistence(self) -> bool:
        """Wait for all data to be persisted."""
        log_info("Waiting for data persistence...")
        time.sleep(10)  # Allow time for flush
        self.assert_true(True, "Waited for persistence")
        return True

    def test_query_trace_list(self) -> bool:
        """Benchmark trace listing query performance."""
        log_section("Performance Tests - Query Benchmarks")
        log_info("Testing trace list query performance...")

        elapsed, result = self._measure_query_time("/traces?limit=100")

        if result and isinstance(result, dict):
            trace_count = len(result.get("traces", []))
            log_success(f"Trace list (100): {elapsed:.3f}s, returned {trace_count} traces")

        passed = elapsed < QUERY_THRESHOLD_TRACE_LIST
        self.assert_true(passed, f"Trace list query < {QUERY_THRESHOLD_TRACE_LIST}s: {elapsed:.3f}s")
        return passed

    def test_query_trace_single(self) -> bool:
        """Benchmark single trace retrieval performance."""
        log_info("Testing single trace query performance...")

        if not self.trace_ids:
            self.skip("No traces to query")
            return True

        # Test multiple random traces
        times = []
        for _ in range(10):
            trace_id = random.choice(self.trace_ids)
            elapsed, _ = self._measure_query_time(f"/traces/{trace_id}")
            times.append(elapsed)

        avg_time = sum(times) / len(times)
        max_time = max(times)

        log_success(f"Single trace query: avg={avg_time:.3f}s, max={max_time:.3f}s")

        passed = avg_time < QUERY_THRESHOLD_TRACE_SINGLE
        self.assert_true(passed, f"Avg single trace query < {QUERY_THRESHOLD_TRACE_SINGLE}s: {avg_time:.3f}s")
        return passed

    def test_query_span_list(self) -> bool:
        """Benchmark span listing query performance."""
        log_info("Testing span list query performance...")

        elapsed, result = self._measure_query_time("/spans?limit=500")

        if result and isinstance(result, list):
            log_success(f"Span list (500): {elapsed:.3f}s, returned {len(result)} spans")

        passed = elapsed < QUERY_THRESHOLD_SPAN_LIST
        self.assert_true(passed, f"Span list query < {QUERY_THRESHOLD_SPAN_LIST}s: {elapsed:.3f}s")
        return passed

    def test_query_spans_by_trace(self) -> bool:
        """Benchmark spans by trace ID query performance."""
        log_info("Testing spans by trace query performance...")

        if not self.trace_ids:
            self.skip("No traces to query")
            return True

        times = []
        for _ in range(10):
            trace_id = random.choice(self.trace_ids)
            elapsed, _ = self._measure_query_time(f"/spans?trace_id={encode_param(trace_id)}&limit=100")
            times.append(elapsed)

        avg_time = sum(times) / len(times)
        log_success(f"Spans by trace query: avg={avg_time:.3f}s")

        passed = avg_time < QUERY_THRESHOLD_SPAN_FILTER
        self.assert_true(passed, f"Spans by trace query < {QUERY_THRESHOLD_SPAN_FILTER}s: {avg_time:.3f}s")
        return passed

    def test_query_filter_by_service(self) -> bool:
        """Benchmark service filter query performance."""
        log_info("Testing service filter query performance...")

        times = []
        for service in self.service_names:
            elapsed, _ = self._measure_query_time(f"/traces?service={encode_param(service)}&limit=100")
            times.append(elapsed)

        avg_time = sum(times) / len(times)
        log_success(f"Service filter query: avg={avg_time:.3f}s")

        passed = avg_time < QUERY_THRESHOLD_SPAN_FILTER
        self.assert_true(passed, f"Service filter query < {QUERY_THRESHOLD_SPAN_FILTER}s: {avg_time:.3f}s")
        return passed

    def test_query_filter_by_model(self) -> bool:
        """Benchmark model filter query performance."""
        log_info("Testing model filter query performance...")

        times = []
        for model in self.models:
            elapsed, _ = self._measure_query_time(f"/spans?model={encode_param(model)}&limit=100")
            times.append(elapsed)

        avg_time = sum(times) / len(times)
        log_success(f"Model filter query: avg={avg_time:.3f}s")

        passed = avg_time < QUERY_THRESHOLD_SPAN_FILTER
        self.assert_true(passed, f"Model filter query < {QUERY_THRESHOLD_SPAN_FILTER}s: {avg_time:.3f}s")
        return passed

    def test_query_pagination(self) -> bool:
        """Benchmark pagination performance through large result sets."""
        log_info("Testing pagination performance...")

        start = time.time()
        cursor = None
        pages = 0
        total_traces = 0
        max_pages = 20

        while pages < max_pages:
            url = "/traces?limit=50"
            if cursor:
                url += f"&cursor={encode_param(cursor)}"

            result = api_call(url)
            if not result or not isinstance(result, dict):
                break

            traces = result.get("traces", [])
            total_traces += len(traces)
            pages += 1

            if not result.get("has_more"):
                break
            cursor = result.get("next_cursor")
            if not cursor:
                break

        elapsed = time.time() - start
        avg_per_page = elapsed / pages if pages > 0 else 0

        log_success(f"Pagination: {pages} pages, {total_traces} traces in {elapsed:.3f}s (avg {avg_per_page:.3f}s/page)")

        passed = avg_per_page < 0.5
        self.assert_true(passed, f"Pagination avg < 0.5s/page: {avg_per_page:.3f}s")
        return passed

    def test_print_summary(self) -> bool:
        """Print performance test summary."""
        log_section("Performance Test Summary")

        ingested_mb = self.total_bytes / (1024 * 1024)
        throughput = ingested_mb / self.ingestion_time if self.ingestion_time > 0 else 0

        log_info(f"Data ingested: {ingested_mb:.1f}MB")
        log_info(f"Total traces: {len(self.trace_ids)}")
        log_info(f"Total spans: {self.total_spans}")
        log_info(f"Ingestion time: {self.ingestion_time:.1f}s")
        log_info(f"Ingestion throughput: {throughput:.1f}MB/s")

        self.assert_true(True, "Performance summary printed")
        return True

    def run_all(self) -> None:
        """Run all performance tests."""
        # Data ingestion
        if not self.test_ingest_large_dataset():
            return
        self.test_wait_for_persistence()

        # Query benchmarks
        self.test_query_trace_list()
        self.test_query_trace_single()
        self.test_query_span_list()
        self.test_query_spans_by_trace()
        self.test_query_filter_by_service()
        self.test_query_filter_by_model()
        self.test_query_pagination()

        # Summary
        self.test_print_summary()
