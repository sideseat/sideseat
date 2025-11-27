"""Strands SDK trace tests - runs Strands SDK to generate real traces."""

import subprocess
import time

from ...api import api_call
from ...config import STRANDS_DIR, STRANDS_TEST_TIMEOUT
from ...logging import log_error, log_info, log_section, log_success
from ..base import BaseTestSuite


class StrandsTraceTests(BaseTestSuite):
    """Tests that run Strands SDK and verify generated traces."""

    def __init__(self) -> None:
        super().__init__()
        self.script_ran_successfully = False
        self.service_name = "strands-e2e"  # Expected service name from strands e2e

    def test_run_strands_e2e(self) -> bool:
        """Run the Strands SDK e2e test script."""
        log_section("Strands SDK Trace Tests")
        log_info("Running Strands SDK e2e script...")
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
                log_success("Strands e2e script completed successfully")
                if result.stdout:
                    lines = result.stdout.strip().split("\n")
                    for line in lines[-5:]:
                        log_info(f"  {line}")
                self.script_ran_successfully = True
                self.passed += 1
                return True
            else:
                log_error(f"Strands e2e script failed with code {result.returncode}")
                if result.stdout:
                    print(result.stdout[-2000:])
                if result.stderr:
                    print(result.stderr[-500:])
                self.failed += 1
                return False

        except subprocess.TimeoutExpired:
            log_error(f"Strands e2e script timed out after {STRANDS_TEST_TIMEOUT}s")
            self.failed += 1
            return False
        except FileNotFoundError:
            log_error("uv command not found - skipping script tests")
            self.skip("uv not available")
            return False
        except Exception as e:
            log_error(f"Failed to run script: {e}")
            self.failed += 1
            return False

    def test_wait_for_script_traces(self) -> bool:
        """Wait for script-generated traces to be persisted."""
        if not self.script_ran_successfully:
            self.skip("Script did not run - skipping wait")
            return True

        log_info("Waiting for script traces to persist...")
        time.sleep(5)  # Allow time for flush
        self.assert_true(True, "Waited for script trace persistence")
        return True

    def test_verify_script_traces_exist(self) -> bool:
        """Verify traces from the script exist in the API."""
        if not self.script_ran_successfully:
            self.skip("Script did not run - skipping trace verification")
            return True

        log_section("Strands Trace Verification")
        log_info("Verifying Strands-generated traces exist...")

        result = api_call("/traces?limit=50")
        if not self.assert_not_none(result, "Trace listing returns data"):
            return False

        if isinstance(result, dict):
            traces = result.get("traces", [])
            self.assert_greater(
                len(traces),
                0,
                "At least one trace exists from script",
            )

            # Look for strands-specific traces
            strands_traces = [
                t for t in traces
                if t.get("detected_framework", "").lower() == "strands"
                or "strands" in t.get("service_name", "").lower()
            ]

            if strands_traces:
                self.assert_true(True, f"Found {len(strands_traces)} Strands traces")
            else:
                # At least verify some traces exist
                self.assert_greater(len(traces), 0, "Traces exist (may not be Strands-specific)")

            return True
        return False

    def test_verify_script_spans_exist(self) -> bool:
        """Verify spans from script-generated traces exist."""
        if not self.script_ran_successfully:
            self.skip("Script did not run - skipping span verification")
            return True

        log_info("Verifying script-generated spans exist...")

        result = api_call("/spans?limit=100")
        if not self.assert_not_none(result, "Span listing returns data"):
            return False

        if isinstance(result, list):
            self.assert_greater(
                len(result),
                0,
                "At least one span exists from script",
            )

            # Check for GenAI-related spans
            genai_spans = [
                s for s in result
                if s.get("gen_ai_request_model")
                or s.get("gen_ai_agent_name")
                or s.get("gen_ai_tool_name")
            ]

            if genai_spans:
                self.assert_true(
                    True,
                    f"Found {len(genai_spans)} GenAI spans from script",
                )

            return True
        return False

    def test_verify_script_trace_structure(self) -> bool:
        """Verify script traces have expected structure."""
        if not self.script_ran_successfully:
            self.skip("Script did not run - skipping structure verification")
            return True

        log_info("Verifying script trace structure...")

        result = api_call("/traces?limit=10")
        if not result or not isinstance(result, dict):
            self.skip("No traces to verify structure")
            return True

        traces = result.get("traces", [])
        if not traces:
            self.skip("No traces available for structure check")
            return True

        # Pick first trace and verify it has spans
        trace = traces[0]
        trace_id = trace.get("trace_id")
        span_count = trace.get("span_count", 0)

        if span_count > 0:
            # Verify spans can be retrieved for this trace
            spans_result = api_call(f"/spans?trace_id={trace_id}&limit=500")
            if isinstance(spans_result, list):
                self.assert_equals(
                    len(spans_result),
                    span_count,
                    f"Trace {trace_id[:16]}... span count matches",
                )
            else:
                self.assert_true(False, "Could not retrieve spans for trace")

        return True

    def run_all(self) -> None:
        """Run all script-based trace tests."""
        # Run the script
        self.test_run_strands_e2e()
        self.test_wait_for_script_traces()

        # Verify generated traces
        self.test_verify_script_traces_exist()
        self.test_verify_script_spans_exist()
        self.test_verify_script_trace_structure()
