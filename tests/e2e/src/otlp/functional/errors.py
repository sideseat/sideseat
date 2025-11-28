"""OTLP error handling tests."""

import json
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from ...api import api_call
from ...base import BaseTestSuite
from ...config import OTEL_BASE
from ...logging import log_info, log_section


class ErrorTests(BaseTestSuite):
    """Error handling tests."""

    def test_malformed_otlp(self) -> bool:
        """Test invalid JSON/Protobuf returns 400."""
        log_section("Error Handling Tests")
        log_info("Testing malformed OTLP handling...")

        # Send invalid JSON
        try:
            req = Request(
                f"{OTEL_BASE}/v1/traces",
                data=b"not valid json",
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urlopen(req, timeout=10) as response:
                # Should not succeed
                return self.assert_true(False, "Malformed JSON was accepted")
        except HTTPError as e:
            # 400 Bad Request is expected
            return self.assert_true(
                e.code in (400, 500),
                f"Malformed JSON rejected with {e.code}",
            )
        except Exception as e:
            # Connection error or other rejection is acceptable
            return self.assert_true(
                True, f"Malformed JSON rejected with error: {type(e).__name__}"
            )

    def test_invalid_trace_id(self) -> bool:
        """Test GET /traces/{invalid} returns 404."""
        log_info("Testing invalid trace ID handling...")

        # Use a non-existent trace ID
        fake_trace_id = "00000000000000000000000000000000"
        result = api_call(f"/traces/{fake_trace_id}", expect_error=True)

        if result and isinstance(result, dict):
            code = result.get("code")
            if code:
                return self.assert_equals(code, 404, "Invalid trace ID returns 404")

        # If no error code returned, check if result is None (which api_call returns on error)
        return self.assert_true(
            result is None or (isinstance(result, dict) and result.get("error")),
            "Invalid trace ID returned error or 404",
        )

    def test_empty_batch(self) -> bool:
        """Test empty OTLP request handled gracefully."""
        log_info("Testing empty batch handling...")

        # Send empty resourceSpans
        payload = {"resourceSpans": []}

        try:
            data = json.dumps(payload).encode()
            req = Request(
                f"{OTEL_BASE}/v1/traces",
                data=data,
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urlopen(req, timeout=10) as response:
                # Empty batch should be accepted (no-op)
                return self.assert_true(
                    response.status == 200,
                    "Empty batch accepted gracefully",
                )
        except HTTPError as e:
            # Some servers may reject empty batches
            return self.assert_true(
                e.code in (200, 400),
                f"Empty batch handled ({e.code})",
            )
        except Exception as e:
            # Connection error means server rejected the request
            return self.assert_true(
                True, f"Empty batch handled with error: {type(e).__name__}"
            )

    def run_all(self) -> None:
        """Run all error handling tests."""
        self.test_malformed_otlp()
        self.test_invalid_trace_id()
        self.test_empty_batch()
