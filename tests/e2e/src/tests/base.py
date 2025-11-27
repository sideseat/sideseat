"""Base test suite with assertion helpers."""

from typing import Any

from ..logging import log_error, log_success, log_warn


class BaseTestSuite:
    """Base class for test suites with assertion helpers."""

    def __init__(self) -> None:
        self.passed = 0
        self.failed = 0
        self.skipped = 0
        self.traces: list[dict[str, Any]] = []
        self.spans: list[dict[str, Any]] = []
        self.all_spans: list[dict[str, Any]] = []

    def assert_true(self, condition: bool, msg: str) -> bool:
        """Assert a condition is true."""
        if condition:
            log_success(msg)
            self.passed += 1
            return True
        else:
            log_error(msg)
            self.failed += 1
            return False

    def assert_equals(self, actual: Any, expected: Any, msg: str) -> bool:
        """Assert two values are equal."""
        if actual == expected:
            log_success(f"{msg}: {actual}")
            self.passed += 1
            return True
        else:
            log_error(f"{msg}: expected {expected}, got {actual}")
            self.failed += 1
            return False

    def assert_greater(self, actual: int, minimum: int, msg: str) -> bool:
        """Assert a value is greater than minimum."""
        if actual > minimum:
            log_success(f"{msg}: {actual} > {minimum}")
            self.passed += 1
            return True
        else:
            log_error(f"{msg}: {actual} not > {minimum}")
            self.failed += 1
            return False

    def assert_not_none(self, value: Any, msg: str) -> bool:
        """Assert a value is not None."""
        if value is not None:
            log_success(msg)
            self.passed += 1
            return True
        else:
            log_error(f"{msg}: got None")
            self.failed += 1
            return False

    def assert_contains(self, haystack: str, needle: str, msg: str) -> bool:
        """Assert haystack contains needle."""
        if needle.lower() in haystack.lower():
            log_success(f"{msg}: '{needle}' found")
            self.passed += 1
            return True
        else:
            log_error(f"{msg}: '{needle}' not found in '{haystack}'")
            self.failed += 1
            return False

    def skip(self, msg: str) -> None:
        """Skip a test with a message."""
        log_warn(f"SKIP: {msg}")
        self.skipped += 1
