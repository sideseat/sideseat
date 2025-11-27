#!/usr/bin/env python3
"""
SideSeat OTEL End-to-End Test Suite

Comprehensive test that:
1. Starts SideSeat server with clean data directory
2. Runs Strands SDK e2e test to generate real trace data
3. Validates ALL OTEL API endpoints with extensive assertions
4. Cleans up all processes
"""

import sys
import time
import traceback
from pathlib import Path

from src import (
    DATA_DIR,
    PROJECT_ROOT,
    SERVER_HOST,
    SERVER_PORT,
    Colors,
    log,
    log_error,
    log_header,
    log_info,
    log_warn,
)
from src.config import TRACE_PERSIST_WAIT
from src.server import clean_data_dir, cleanup_server, start_server, wait_for_server
from src.strands import run_strands_e2e
from src.tests import BaseTestSuite, HealthTests, IntegrityTests, SpanTests, TraceTests


def verify_storage_files() -> tuple[int, int]:
    """Verify storage files exist and have data after server shutdown.

    Returns tuple of (passed, failed) counts.
    """
    log_header("Storage Verification (Post-Shutdown)")
    passed = 0
    failed = 0

    # Check SQLite database
    db_path = DATA_DIR / "traces" / "traces.db"
    if db_path.exists() and db_path.stat().st_size > 0:
        log(f"  {Colors.GREEN}✓ SQLite database exists: {db_path.stat().st_size} bytes{Colors.RESET}")
        passed += 1
    else:
        log(f"  {Colors.RED}✗ SQLite database missing or empty{Colors.RESET}")
        failed += 1

    # Check parquet files
    traces_dir = DATA_DIR / "traces"
    parquet_files = list(traces_dir.rglob("*.parquet"))
    if parquet_files:
        total_size = sum(f.stat().st_size for f in parquet_files)
        non_empty = [f for f in parquet_files if f.stat().st_size > 0]
        if non_empty:
            log(f"  {Colors.GREEN}✓ Parquet files: {len(non_empty)} files, {total_size} bytes{Colors.RESET}")
            passed += 1
        else:
            log(f"  {Colors.RED}✗ Parquet files exist but are empty (0 bytes){Colors.RESET}")
            failed += 1
    else:
        log(f"  {Colors.RED}✗ No parquet files found{Colors.RESET}")
        failed += 1

    return passed, failed


class OtelApiTests:
    """Comprehensive OTEL API test suite."""

    def __init__(self) -> None:
        self.passed = 0
        self.failed = 0
        self.skipped = 0

    def run_all_tests(self) -> bool:
        """Run all tests."""
        log_header("Running OTEL API Tests")

        # Wait for traces to be persisted
        log_info(f"Waiting {TRACE_PERSIST_WAIT}s for traces to persist...")
        time.sleep(TRACE_PERSIST_WAIT)

        # Run health tests
        health = HealthTests()
        health.run_all()
        self._collect_results(health)

        # Run trace tests
        traces = TraceTests()
        traces.run_all()
        self._collect_results(traces)

        # Run span tests (share trace data from trace tests)
        spans = SpanTests()
        spans.traces = traces.traces
        spans.run_all()
        self._collect_results(spans)

        # Run integrity tests (share data from previous tests)
        integrity = IntegrityTests()
        integrity.traces = traces.traces
        integrity.spans = spans.spans
        integrity.all_spans = spans.all_spans
        integrity.run_all()
        self._collect_results(integrity)

        return self.failed == 0

    def _collect_results(self, suite: BaseTestSuite) -> None:
        """Collect results from a test suite."""
        self.passed += suite.passed
        self.failed += suite.failed
        self.skipped += suite.skipped

    def print_summary(self) -> None:
        """Print test summary."""
        log_header("Test Summary")
        total = self.passed + self.failed + self.skipped
        log(f"  Total:   {total} tests")
        log(f"  {Colors.GREEN}✓ Passed:  {self.passed}{Colors.RESET}")
        if self.skipped > 0:
            log(f"  {Colors.YELLOW}⚠ Skipped: {self.skipped}{Colors.RESET}")
        if self.failed > 0:
            log(f"  {Colors.RED}✗ Failed:  {self.failed}{Colors.RESET}")
        else:
            log(f"\n  {'*' * 50}", Colors.GREEN)
            log("  *  ALL TESTS PASSED!  *", Colors.GREEN + Colors.BOLD)
            log(f"  {'*' * 50}", Colors.GREEN)


def main() -> int:
    """Main test runner."""
    log_header("SideSeat OTEL End-to-End Test Suite")
    log(f"  Project root:    {PROJECT_ROOT}")
    log(f"  Data directory:  {DATA_DIR}")
    log(f"  Server:          {SERVER_HOST}:{SERVER_PORT}")

    server_proc = None
    exit_code = 1

    try:
        # Step 1: Clean data directory
        clean_data_dir()

        # Step 2: Start server
        server_proc = start_server()
        if not server_proc:
            return 1

        # Step 3: Wait for server
        if not wait_for_server():
            return 1

        # Step 4: Run Strands e2e test
        strands_ok = run_strands_e2e()
        if not strands_ok:
            log_warn("Strands e2e test failed - continuing with API tests anyway")

        # Step 5: Run comprehensive OTEL API tests
        api_tests = OtelApiTests()
        all_passed = api_tests.run_all_tests()

        # Success if API tests pass (Strands failure is a warning)
        exit_code = 0 if all_passed else 1

    except KeyboardInterrupt:
        log_warn("\nTest interrupted by user")
        exit_code = 130
    except Exception as e:
        log_error(f"Unexpected error: {e}")
        traceback.print_exc()
        exit_code = 1
    finally:
        cleanup_server(server_proc)

        # Verify storage files after server shutdown (tests graceful shutdown)
        if exit_code == 0:
            storage_passed, storage_failed = verify_storage_files()
            api_tests.passed += storage_passed
            api_tests.failed += storage_failed
            if storage_failed > 0:
                exit_code = 1
            api_tests.print_summary()

    return exit_code


if __name__ == "__main__":
    sys.exit(main())
