#!/usr/bin/env python3
"""
SideSeat E2E Test Suite

Configurable test runner that supports:
- Health and infrastructure tests
- Synthetic trace ingestion
- Strands SDK trace generation
- API validation tests
- Performance benchmarks

Configure tests in config.yaml or via CLI arguments.

Usage:
  uv run test                    # Run all tests (from config.yaml)
  uv run test health             # Run only health tests
  uv run test health synthetic   # Run health and synthetic tests
  uv run test --list             # List available tests
  uv run test --all              # Run all tests (ignore config.yaml)
"""

import argparse
import sys
import time
import traceback
from pathlib import Path
from typing import Any

import yaml

# Available test suites
AVAILABLE_TESTS = ["health", "synthetic", "strands", "traces", "spans", "integrity", "performance", "sse"]

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
from src.otel import (
    BaseTestSuite,
    HealthTests,
    IntegrityTests,
    PerformanceTests,
    SpanTests,
    SSETests,
    StrandsTraceTests,
    SyntheticTraceTests,
    TraceTests,
)
from src.server import clean_data_dir, cleanup_server, start_server, wait_for_server

# Default configuration
DEFAULT_CONFIG = {
    "tests": {
        "otel": {
            "health": True,
            "synthetic": True,
            "strands": True,
            "traces": True,
            "spans": True,
            "integrity": True,
            "performance": True,
            "sse": True,
            "performance_settings": {
                "target_size_mb": 200,
                "batch_size": 100,
                "spans_per_trace": 20,
            },
            "thresholds": {
                "trace_list": 1.0,
                "trace_single": 0.5,
                "span_list": 1.0,
                "span_filter": 1.5,
                "pagination_per_page": 0.5,
            },
            "trace_persist_wait": 10,
        },
    },
}


def load_config() -> dict[str, Any]:
    """Load configuration from config.yaml."""
    config_path = Path(__file__).parent / "config.yaml"

    if config_path.exists():
        try:
            with open(config_path) as f:
                user_config = yaml.safe_load(f) or {}

            # Merge with defaults
            config = {"tests": {"otel": DEFAULT_CONFIG["tests"]["otel"].copy()}}

            if "tests" in user_config and "otel" in user_config["tests"]:
                otel_config = user_config["tests"]["otel"]
                for key, value in otel_config.items():
                    if isinstance(value, dict) and key in config["tests"]["otel"]:
                        config["tests"]["otel"][key].update(value)
                    else:
                        config["tests"]["otel"][key] = value

            return config
        except Exception as e:
            log_warn(f"Failed to load config.yaml: {e}, using defaults")

    return DEFAULT_CONFIG


def verify_storage_files(expect_traces: bool = True) -> tuple[int, int]:
    """Verify storage files exist and have data after server shutdown.

    Args:
        expect_traces: If True, verify parquet files exist (traces were ingested).
                      If False, only verify SQLite database exists.
    """
    log_header("Storage Verification (Post-Shutdown)")
    passed = 0
    failed = 0

    # Check SQLite database
    db_path = DATA_DIR / "traces" / "traces.db"
    if db_path.exists() and db_path.stat().st_size > 0:
        size_bytes = db_path.stat().st_size
        if size_bytes > 1024 * 1024:
            size_str = f"{size_bytes / (1024 * 1024):.1f}MB"
        else:
            size_str = f"{size_bytes} bytes"
        log(f"  {Colors.GREEN}✓ SQLite database exists: {size_str}{Colors.RESET}")
        passed += 1
    else:
        log(f"  {Colors.RED}✗ SQLite database missing or empty{Colors.RESET}")
        failed += 1

    # Check parquet files only if traces were expected
    if expect_traces:
        traces_dir = DATA_DIR / "traces"
        parquet_files = list(traces_dir.rglob("*.parquet"))
        if parquet_files:
            total_size = sum(f.stat().st_size for f in parquet_files)
            if total_size > 1024 * 1024:
                size_str = f"{total_size / (1024 * 1024):.1f}MB"
            else:
                size_str = f"{total_size} bytes"
            non_empty = [f for f in parquet_files if f.stat().st_size > 0]
            if non_empty:
                log(f"  {Colors.GREEN}✓ Parquet files: {len(non_empty)} files, {size_str}{Colors.RESET}")
                passed += 1
            else:
                log(f"  {Colors.RED}✗ Parquet files exist but are empty (0 bytes){Colors.RESET}")
                failed += 1
        else:
            log(f"  {Colors.RED}✗ No parquet files found{Colors.RESET}")
            failed += 1
    else:
        log(f"  {Colors.YELLOW}⚠ Skipping parquet check (no trace tests ran){Colors.RESET}")

    return passed, failed


class TestRunner:
    """Configurable test runner."""

    def __init__(self, config: dict[str, Any]) -> None:
        self.config = config
        self.passed = 0
        self.failed = 0
        self.skipped = 0
        self.traces: list[dict[str, Any]] = []
        self.spans: list[dict[str, Any]] = []
        self.all_spans: list[dict[str, Any]] = []

    def _collect_results(self, suite: BaseTestSuite) -> None:
        """Collect results from a test suite."""
        self.passed += suite.passed
        self.failed += suite.failed
        self.skipped += suite.skipped

    def _is_enabled(self, category: str, test_name: str) -> bool:
        """Check if a test is enabled."""
        return self.config.get("tests", {}).get(category, {}).get(test_name, False)

    def run_all_tests(self) -> bool:
        """Run all enabled tests."""
        log_header("Running E2E Tests")
        otel_config = self.config.get("tests", {}).get("otel", {})
        enabled = [k for k, v in otel_config.items() if v is True]
        log_info(f"Enabled OTEL tests: {', '.join(enabled)}")

        # Wait for traces to be persisted
        wait_time = otel_config.get("trace_persist_wait", 10)
        log_info(f"Waiting {wait_time}s for traces to persist...")
        time.sleep(wait_time)

        # Health tests
        if self._is_enabled("otel", "health"):
            health = HealthTests()
            health.run_all()
            self._collect_results(health)

        # Synthetic trace tests
        if self._is_enabled("otel", "synthetic"):
            synthetic = SyntheticTraceTests()
            synthetic.run_all()
            self._collect_results(synthetic)

        # Strands SDK tests
        if self._is_enabled("otel", "strands"):
            strands = StrandsTraceTests()
            strands.run_all()
            self._collect_results(strands)

        # Trace tests
        traces_suite = None
        if self._is_enabled("otel", "traces"):
            traces_suite = TraceTests()
            traces_suite.run_all()
            self._collect_results(traces_suite)
            self.traces = traces_suite.traces

        # Span tests
        spans_suite = None
        if self._is_enabled("otel", "spans"):
            spans_suite = SpanTests()
            if traces_suite:
                spans_suite.traces = traces_suite.traces
            spans_suite.run_all()
            self._collect_results(spans_suite)
            self.spans = spans_suite.spans
            self.all_spans = spans_suite.all_spans

        # Integrity tests
        if self._is_enabled("otel", "integrity"):
            integrity = IntegrityTests()
            if traces_suite:
                integrity.traces = traces_suite.traces
            if spans_suite:
                integrity.spans = spans_suite.spans
                integrity.all_spans = spans_suite.all_spans
            integrity.run_all()
            self._collect_results(integrity)

        # Performance tests
        if self._is_enabled("otel", "performance"):
            perf = PerformanceTests()
            # Apply performance settings from nested otel config
            perf_settings = otel_config.get("performance_settings", {})
            from src.otel.performance import tests as perf_module
            if "target_size_mb" in perf_settings:
                perf_module.TARGET_SIZE_MB = perf_settings["target_size_mb"]
            if "batch_size" in perf_settings:
                perf_module.BATCH_SIZE = perf_settings["batch_size"]
            if "spans_per_trace" in perf_settings:
                perf_module.SPANS_PER_TRACE = perf_settings["spans_per_trace"]

            # Apply thresholds from nested otel config
            thresholds = otel_config.get("thresholds", {})
            if "trace_list" in thresholds:
                perf_module.QUERY_THRESHOLD_TRACE_LIST = thresholds["trace_list"]
            if "trace_single" in thresholds:
                perf_module.QUERY_THRESHOLD_TRACE_SINGLE = thresholds["trace_single"]
            if "span_list" in thresholds:
                perf_module.QUERY_THRESHOLD_SPAN_LIST = thresholds["span_list"]
            if "span_filter" in thresholds:
                perf_module.QUERY_THRESHOLD_SPAN_FILTER = thresholds["span_filter"]

            perf.run_all()
            self._collect_results(perf)

        # SSE tests
        if self._is_enabled("otel", "sse"):
            sse = SSETests()
            sse.run_all()
            self._collect_results(sse)

        return self.failed == 0

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


def parse_args() -> argparse.Namespace:
    """Parse command line arguments."""
    parser = argparse.ArgumentParser(
        description="SideSeat E2E Test Suite",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  uv run test                    Run tests as configured in config.yaml
  uv run test health             Run only health tests
  uv run test health synthetic   Run health and synthetic tests
  uv run test --all              Run all tests (ignore config.yaml)
  uv run test --list             List available test suites

Available test suites:
  health      - Health and infrastructure tests
  synthetic   - Synthetic trace ingestion and verification
  strands     - Strands SDK trace generation and verification
  traces      - Trace API tests (listing, filtering, pagination)
  spans       - Span API tests (listing, filtering)
  integrity   - Data integrity tests (framework detection, token usage)
  performance - Performance tests (~200MB data ingestion and benchmarks)
  sse         - SSE real-time event streaming tests (latency, filtering)
""",
    )
    parser.add_argument(
        "tests",
        nargs="*",
        choices=AVAILABLE_TESTS,
        metavar="TEST",
        help=f"Test suites to run. Available: {', '.join(AVAILABLE_TESTS)}",
    )
    parser.add_argument(
        "--all",
        action="store_true",
        help="Run all tests (ignore config.yaml settings)",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="List available test suites and exit",
    )
    return parser.parse_args()


def apply_cli_args(config: dict[str, Any], args: argparse.Namespace) -> dict[str, Any]:
    """Apply CLI arguments to config."""
    if args.list:
        return config  # Will be handled in main

    # If specific tests are provided via CLI, disable all others
    if args.tests:
        for test in AVAILABLE_TESTS:
            config["tests"]["otel"][test] = test in args.tests
    elif args.all:
        # Enable all tests
        for test in AVAILABLE_TESTS:
            config["tests"]["otel"][test] = True

    return config


def main() -> int:
    """Main test runner."""
    # Parse CLI arguments
    args = parse_args()

    # Handle --list
    if args.list:
        print("Available test suites:")
        print("  health      - Health and infrastructure tests")
        print("  synthetic   - Synthetic trace ingestion and verification")
        print("  strands     - Strands SDK trace generation and verification")
        print("  traces      - Trace API tests (listing, filtering, pagination)")
        print("  spans       - Span API tests (listing, filtering)")
        print("  integrity   - Data integrity tests (framework detection, token usage)")
        print("  performance - Performance tests (~200MB data ingestion and benchmarks)")
        print("  sse         - SSE real-time event streaming tests (latency, filtering)")
        return 0

    # Load configuration and apply CLI args
    config = load_config()
    config = apply_cli_args(config, args)

    log_header("SideSeat E2E Test Suite")
    log(f"  Project root:    {PROJECT_ROOT}")
    log(f"  Data directory:  {DATA_DIR}")
    log(f"  Server:          {SERVER_HOST}:{SERVER_PORT}")

    server_proc = None
    exit_code = 1
    runner = None

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

        # Step 4: Run tests
        runner = TestRunner(config)
        all_passed = runner.run_all_tests()

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

        # Verify storage files after server shutdown
        if runner and exit_code == 0:
            # Only expect traces if tests that generate them were run
            otel_config = config.get("tests", {}).get("otel", {})
            trace_tests = ["synthetic", "strands", "performance", "sse"]
            expect_traces = any(otel_config.get(t, False) for t in trace_tests)
            storage_passed, storage_failed = verify_storage_files(expect_traces)
            runner.passed += storage_passed
            runner.failed += storage_failed
            if storage_failed > 0:
                exit_code = 1

        if runner:
            runner.print_summary()

    return exit_code


if __name__ == "__main__":
    sys.exit(main())
