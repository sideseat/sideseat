#!/usr/bin/env python3
"""
SideSeat E2E Test Suite

Configurable test runner with namespace-based organization:
- otlp/smoke: Quick validation tests (~30 seconds)
- otlp/functional: Comprehensive feature tests (~5 minutes)
- otlp/performance: Load and query benchmarks (~10 minutes)

Configure tests in config.yaml or via CLI arguments.

Usage:
  uv run test                       # Run all tests (from config.yaml)
  uv run test otlp.smoke            # Run only smoke tests
  uv run test otlp.functional       # Run only functional tests
  uv run test otlp.performance      # Run only performance tests
  uv run test --list                # List available test phases
  uv run test --all                 # Run all tests (ignore config.yaml)
"""

import argparse
import sys
import time
import traceback
from pathlib import Path
from typing import Any

import yaml

# Available test phases
AVAILABLE_PHASES = ["otlp.smoke", "otlp.functional", "otlp.performance"]

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
    log_section,
    log_warn,
)
from src.base import BaseTestSuite
from src.server import (
    clean_data_dir,
    cleanup_server,
    get_bootstrap_token,
    start_server,
    wait_for_server,
)

# Default configuration
DEFAULT_CONFIG = {
    "tests": {
        "otlp": {
            "smoke": True,
            "functional": True,
            "performance": True,
        },
    },
    "settings": {
        "trace_persist_wait": 3,
        "api_timeout": 30,
        "sse_timeout": 10,
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
            config = {
                "tests": {"otlp": DEFAULT_CONFIG["tests"]["otlp"].copy()},
                "settings": DEFAULT_CONFIG["settings"].copy(),
            }

            if "tests" in user_config and "otlp" in user_config["tests"]:
                config["tests"]["otlp"].update(user_config["tests"]["otlp"])

            if "settings" in user_config:
                config["settings"].update(user_config["settings"])

            return config
        except Exception as e:
            log_warn(f"Failed to load config.yaml: {e}, using defaults")

    return DEFAULT_CONFIG


def verify_storage_files(expect_traces: bool = True) -> tuple[int, int]:
    """Verify storage files exist and have data after server shutdown."""
    log_header("Storage Verification (Post-Shutdown)")
    passed = 0
    failed = 0

    # Check SQLite database
    db_path = DATA_DIR / "sideseat.db"
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

    # Storage verification complete (SQLite-only)

    return passed, failed


class TestRunner:
    """Configurable test runner for namespace-based test organization."""

    def __init__(self, config: dict[str, Any]) -> None:
        self.config = config
        self.passed = 0
        self.failed = 0
        self.skipped = 0

    def _collect_results(self, suite: BaseTestSuite) -> None:
        """Collect results from a test suite."""
        self.passed += suite.passed
        self.failed += suite.failed
        self.skipped += suite.skipped

    def _is_enabled(self, namespace: str, phase: str) -> bool:
        """Check if a test phase is enabled."""
        return self.config.get("tests", {}).get(namespace, {}).get(phase, False)

    def run_smoke_tests(self) -> None:
        """Run OTLP smoke tests."""
        log_section("OTLP Smoke Tests")
        from src.otlp.smoke import SmokeTests

        suite = SmokeTests()
        suite.run_all()
        self._collect_results(suite)

    def run_functional_tests(self) -> None:
        """Run OTLP functional tests."""
        log_section("OTLP Functional Tests")
        from src.otlp.functional import (
            APITests,
            AuthTests,
            ErrorTests,
            IngestionTests,
            IntegrityTests,
            LimitsTests,
            SessionTests,
            ShutdownTests,
            SSETests,
        )

        # Run in logical order
        test_suites = [
            IngestionTests(),
            APITests(),
            SessionTests(),
            SSETests(),
            IntegrityTests(),
            ErrorTests(),
            LimitsTests(),
            AuthTests(),
            ShutdownTests(),
        ]

        for suite in test_suites:
            suite.run_all()
            self._collect_results(suite)

    def run_performance_tests(self) -> None:
        """Run OTLP performance tests."""
        log_section("OTLP Performance Tests")
        from src.otlp.performance import LoadTests, QueryTests

        load_tests = LoadTests()
        load_tests.run_all()
        self._collect_results(load_tests)

        query_tests = QueryTests()
        query_tests.run_all()
        self._collect_results(query_tests)

    def run_all_tests(self) -> bool:
        """Run all enabled tests."""
        log_header("Running E2E Tests")

        otlp_config = self.config.get("tests", {}).get("otlp", {})
        enabled = [k for k, v in otlp_config.items() if v is True]
        log_info(f"Enabled OTLP phases: {', '.join(enabled)}")

        # Wait for server initialization
        wait_time = self.config.get("settings", {}).get("trace_persist_wait", 3)
        log_info(f"Waiting {wait_time}s for server initialization...")
        time.sleep(wait_time)

        # Run smoke tests first
        if self._is_enabled("otlp", "smoke"):
            self.run_smoke_tests()

        # Run functional tests
        if self._is_enabled("otlp", "functional"):
            self.run_functional_tests()

        # Run performance tests last
        if self._is_enabled("otlp", "performance"):
            self.run_performance_tests()

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
  uv run test                       Run tests as configured in config.yaml
  uv run test otlp.smoke            Run only smoke tests
  uv run test otlp.functional       Run only functional tests
  uv run test otlp.performance      Run only performance tests
  uv run test --all                 Run all tests (ignore config.yaml)
  uv run test --list                List available test phases

Test Phases:
  otlp.smoke        - Quick validation (~30 seconds)
                      Health, basic ingestion, endpoint availability
  otlp.functional   - Comprehensive feature tests (~5 minutes)
                      Ingestion, API, SSE, auth, integrity,
                      errors, limits, shutdown
  otlp.performance  - Load and query benchmarks (~10 minutes)
                      Throughput, concurrency, latency measurements
""",
    )
    parser.add_argument(
        "phases",
        nargs="*",
        choices=AVAILABLE_PHASES,
        metavar="PHASE",
        help=f"Test phases to run. Available: {', '.join(AVAILABLE_PHASES)}",
    )
    parser.add_argument(
        "--all",
        action="store_true",
        help="Run all tests (ignore config.yaml settings)",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="List available test phases and exit",
    )
    parser.add_argument(
        "--no-auth",
        action="store_true",
        help="Start server with authentication disabled",
    )
    return parser.parse_args()


def apply_cli_args(config: dict[str, Any], args: argparse.Namespace) -> dict[str, Any]:
    """Apply CLI arguments to config."""
    if args.list:
        return config

    # Map phase names to config keys
    phase_map = {
        "otlp.smoke": ("otlp", "smoke"),
        "otlp.functional": ("otlp", "functional"),
        "otlp.performance": ("otlp", "performance"),
    }

    if args.phases:
        # Disable all, then enable selected
        for phase in phase_map.values():
            config["tests"][phase[0]][phase[1]] = False
        for phase in args.phases:
            ns, name = phase_map[phase]
            config["tests"][ns][name] = True
    elif args.all:
        for ns, name in phase_map.values():
            config["tests"][ns][name] = True

    return config


def main() -> int:
    """Main test runner."""
    args = parse_args()

    if args.list:
        print("Available test phases:")
        print("  otlp.smoke        - Quick validation (~30 seconds)")
        print("                      Health, basic ingestion, endpoint availability")
        print("  otlp.functional   - Comprehensive feature tests (~5 minutes)")
        print("                      Ingestion, API, SSE, auth, integrity,")
        print("                      errors, limits, shutdown")
        print("  otlp.performance  - Load and query benchmarks (~10 minutes)")
        print("                      Throughput, concurrency, latency measurements")
        return 0

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
        server_proc = start_server(no_auth=args.no_auth)
        if not server_proc:
            return 1

        # Step 3: Wait for server (pass proc to capture bootstrap token)
        if not wait_for_server(server_proc=server_proc):
            return 1

        # Log auth mode
        if not args.no_auth:
            token = get_bootstrap_token()
            if token:
                log_info(f"Auth enabled with bootstrap token: {token[:8]}...")
            else:
                log_warn("Auth enabled but no bootstrap token captured")

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
            otlp_config = config.get("tests", {}).get("otlp", {})
            # Expect traces if functional or performance tests ran
            expect_traces = otlp_config.get("functional", False) or otlp_config.get(
                "performance", False
            )
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
