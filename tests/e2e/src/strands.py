"""Strands SDK e2e test runner."""

import subprocess

from .config import STRANDS_DIR, STRANDS_TEST_TIMEOUT
from .logging import log_error, log_header, log_info, log_success


def run_strands_e2e() -> bool:
    """Run the Strands SDK e2e test."""
    log_header("Running Strands E2E Test")
    log_info("This may take several minutes...")

    try:
        result = subprocess.run(
            ["uv", "run", "e2e"],
            cwd=STRANDS_DIR,
            timeout=STRANDS_TEST_TIMEOUT,
            capture_output=True,
            text=True,
        )

        if result.returncode == 0:
            log_success("Strands e2e test completed successfully")
            # Print some output for visibility
            if result.stdout:
                lines = result.stdout.strip().split("\n")
                for line in lines[-10:]:  # Last 10 lines
                    log_info(f"  {line}")
            return True
        else:
            log_error(f"Strands e2e test failed with code {result.returncode}")
            if result.stdout:
                print(result.stdout[-3000:])
            if result.stderr:
                print(result.stderr[-1000:])
            return False

    except subprocess.TimeoutExpired:
        log_error(f"Strands e2e test timed out after {STRANDS_TEST_TIMEOUT}s")
        return False
    except FileNotFoundError:
        log_error("uv command not found. Install from https://docs.astral.sh/uv/")
        return False
    except Exception as e:
        log_error(f"Failed to run Strands e2e test: {e}")
        return False
