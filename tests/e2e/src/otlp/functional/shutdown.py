"""OTLP graceful shutdown tests.

These tests verify that data is properly persisted and the server
handles shutdown gracefully. Note: Full shutdown testing requires
coordination with the test_runner which manages the server process.
"""

import socket
import time

from ...api import api_call
from ...base import BaseTestSuite
from ...config import DATA_DIR, GRPC_PORT, SERVER_HOST, TRACE_PERSIST_WAIT
from ...logging import log_info, log_section, log_warn
from ..traces import create_batch_traces, create_simple_trace, send_otlp_traces_http


class ShutdownTests(BaseTestSuite):
    """Graceful shutdown behavior tests.

    Note: The actual graceful shutdown (SIGTERM handling, WAL checkpoint)
    is verified by the test_runner after all tests complete. These tests
    verify prerequisites for proper shutdown behavior.
    """

    def __init__(self) -> None:
        super().__init__()
        self.pre_shutdown_trace_ids: list[str] = []

    def test_no_data_loss(self) -> bool:
        """Test that traces ingested are persisted and queryable.

        This test ingests data and verifies it can be queried back,
        establishing a baseline for the post-shutdown verification.
        """
        log_section("Shutdown Tests")
        log_info("Testing data persistence (pre-shutdown baseline)...")

        # Ingest several traces
        traces = create_batch_traces(count=5, spans_per_trace=3)
        success_count = 0

        for trace_id, payload in traces:
            success, _ = send_otlp_traces_http(payload)
            if success:
                self.pre_shutdown_trace_ids.append(trace_id)
                success_count += 1

        if not self.assert_equals(success_count, 5, "All 5 traces ingested"):
            return False

        # Wait for persistence
        time.sleep(TRACE_PERSIST_WAIT)

        # Verify all traces are queryable
        found = 0
        for trace_id in self.pre_shutdown_trace_ids:
            result = api_call(f"/traces/{trace_id}")
            if result and isinstance(result, dict):
                if result.get("trace_id") == trace_id:
                    found += 1

        return self.assert_equals(found, 5, "All 5 traces persisted and queryable")

    def test_pending_writes_flush(self) -> bool:
        """Test that the buffer flushes writes properly.

        Verifies that traces sent in quick succession are all persisted,
        indicating the buffer/write system works correctly.
        """
        log_info("Testing pending writes flush...")

        # Send traces in rapid succession (simulating pending writes)
        trace_ids = []
        for i in range(10):
            trace_id, payload = create_simple_trace(
                service_name=f"flush-test-{i}",
                span_count=2,
            )
            success, _ = send_otlp_traces_http(payload)
            if success:
                trace_ids.append(trace_id)

        # Wait for buffer to flush
        time.sleep(TRACE_PERSIST_WAIT + 2)

        # Verify all traces persisted
        found = 0
        for trace_id in trace_ids:
            result = api_call(f"/traces/{trace_id}")
            if result:
                found += 1

        return self.assert_equals(found, 10, f"Buffer flushed all {found}/10 traces")

    def test_wal_checkpoint_readiness(self) -> bool:
        """Verify SQLite database is accessible and ready for checkpoint.

        Checks that the database file exists and is being written to,
        which is a prerequisite for WAL checkpointing on shutdown.
        """
        log_info("Testing WAL checkpoint readiness...")

        db_path = DATA_DIR / "sideseat.db"
        wal_path = DATA_DIR / "sideseat.db-wal"

        # Check database exists
        if not db_path.exists():
            log_warn("Database file not found yet, may not be initialized")
            # Try to trigger database creation by making a query
            api_call("/traces?limit=1")
            time.sleep(2)

        if db_path.exists():
            db_size = db_path.stat().st_size
            self.assert_greater(
                int(db_size), 0, f"Database file exists ({db_size} bytes)"
            )

            # Check if WAL file exists (indicates WAL mode is active)
            if wal_path.exists():
                wal_size = wal_path.stat().st_size
                log_info(f"WAL file exists ({wal_size} bytes) - WAL mode active")

            return self.assert_true(True, "Database ready for shutdown checkpoint")

        return self.assert_true(False, "Database file not created")

    def test_server_responsive_under_load(self) -> bool:
        """Test server remains responsive during heavy writes.

        Verifies the server doesn't become unresponsive during data
        ingestion, which is important for graceful shutdown handling.
        """
        log_info("Testing server responsiveness under load...")

        # Start sending traces
        traces = create_batch_traces(count=20, spans_per_trace=5)

        # Track health check responsiveness during ingestion
        health_checks_passed = 0
        ingestion_count = 0

        for i, (trace_id, payload) in enumerate(traces):
            # Ingest trace
            success, _ = send_otlp_traces_http(payload)
            if success:
                ingestion_count += 1

            # Check health every 5 traces
            if i % 5 == 4:
                result = api_call("/health")
                if result and isinstance(result, dict):
                    if result.get("status", "").lower() in ("ok", "healthy"):
                        health_checks_passed += 1

        self.assert_greater(
            ingestion_count,
            15,
            f"Ingestion during load: {ingestion_count}/20",
        )

        return self.assert_greater(
            health_checks_passed,
            2,
            f"Health checks during load: {health_checks_passed}/4",
        )

    def test_connections_closeable(self) -> bool:
        """Test that connections can be properly closed.

        Verifies both HTTP and gRPC connections can be established
        and closed cleanly, which is required for graceful shutdown.
        """
        log_info("Testing connection cleanup capability...")

        # Test HTTP connection close
        http_success = False
        result = api_call("/health")
        if result:
            http_success = True

        # Test gRPC port availability
        grpc_available = False
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.settimeout(2)
            result = sock.connect_ex((SERVER_HOST, GRPC_PORT))
            sock.close()  # Clean close
            grpc_available = result == 0
        except Exception:
            pass

        http_ok = self.assert_true(http_success, "HTTP connection closeable")
        grpc_ok = self.assert_true(grpc_available, "gRPC port accessible")

        return http_ok and grpc_ok

    def test_graceful_shutdown_prerequisites(self) -> bool:
        """Verify all prerequisites for graceful shutdown are met.

        This is a summary test that verifies the server is in a state
        where graceful shutdown should work correctly.
        """
        log_info("Verifying graceful shutdown prerequisites...")

        checks_passed = 0
        total_checks = 4

        # 1. Server is healthy
        result = api_call("/health")
        if result and isinstance(result, dict):
            status = result.get("status", "").lower()
            if status in ("ok", "healthy"):
                checks_passed += 1
                log_info("  [OK] Server healthy")
            else:
                log_warn(f"  [FAIL] Server status: {status}")
        else:
            log_warn("  [FAIL] Health check failed")

        # 2. OTLP collector is enabled
        if result and isinstance(result, dict):
            otel = result.get("otel", {})
            if otel.get("enabled"):
                checks_passed += 1
                log_info("  [OK] OTLP collector enabled")
            else:
                log_warn("  [FAIL] OTLP collector not enabled")

        # 3. Database file exists
        db_path = DATA_DIR / "sideseat.db"
        if db_path.exists():
            checks_passed += 1
            log_info("  [OK] Database file exists")
        else:
            log_warn("  [FAIL] Database file missing")

        # 4. Can query traces
        result = api_call("/traces?limit=1")
        if result and isinstance(result, dict):
            checks_passed += 1
            log_info("  [OK] Trace queries working")
        else:
            log_warn("  [FAIL] Trace queries failing")

        return self.assert_equals(
            checks_passed,
            total_checks,
            f"Shutdown prerequisites: {checks_passed}/{total_checks}",
        )

    def run_all(self) -> None:
        """Run all shutdown tests."""
        self.test_no_data_loss()
        self.test_pending_writes_flush()
        self.test_wal_checkpoint_readiness()
        self.test_server_responsive_under_load()
        self.test_connections_closeable()
        self.test_graceful_shutdown_prerequisites()
