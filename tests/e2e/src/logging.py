"""Logging utilities for e2e tests."""


class Colors:
    """ANSI color codes for terminal output."""

    GREEN = "\033[92m"
    RED = "\033[91m"
    YELLOW = "\033[93m"
    BLUE = "\033[94m"
    CYAN = "\033[96m"
    RESET = "\033[0m"
    BOLD = "\033[1m"


def log(msg: str, color: str = "") -> None:
    """Print a log message with optional color."""
    print(f"{color}{msg}{Colors.RESET}")


def log_success(msg: str) -> None:
    """Log a success message."""
    log(f"  ✓ {msg}", Colors.GREEN)


def log_error(msg: str) -> None:
    """Log an error message."""
    log(f"  ✗ {msg}", Colors.RED)


def log_info(msg: str) -> None:
    """Log an info message."""
    log(f"  → {msg}", Colors.BLUE)


def log_warn(msg: str) -> None:
    """Log a warning message."""
    log(f"  ⚠ {msg}", Colors.YELLOW)


def log_header(msg: str) -> None:
    """Log a header message."""
    log(f"\n{'=' * 70}\n  {msg}\n{'=' * 70}", Colors.BOLD)


def log_section(msg: str) -> None:
    """Log a section message."""
    log(f"\n  {'-' * 50}\n  {msg}\n  {'-' * 50}", Colors.CYAN)
