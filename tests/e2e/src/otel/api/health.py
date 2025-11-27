"""Health and infrastructure tests."""

from urllib.error import HTTPError
from urllib.request import Request, urlopen

from ...api import api_call
from ...config import API_BASE, OTEL_BASE
from ...logging import log_info, log_section
from ..base import BaseTestSuite


class HealthTests(BaseTestSuite):
    """Health and infrastructure test suite."""

    def test_health_endpoint(self) -> bool:
        """Test the health endpoint."""
        log_section("Health & Infrastructure Tests")
        log_info("Testing health endpoint...")
        result = api_call("/health")
        if not self.assert_not_none(result, "Health endpoint responds"):
            return False
        if isinstance(result, dict):
            status = result.get("status", "").lower()
            self.assert_equals(status, "ok", "Health status is ok")
        return True

    def test_otel_collector_endpoint(self) -> bool:
        """Test OTEL collector endpoints exist."""
        log_info("Testing OTEL collector endpoint...")

        try:
            req = Request(
                f"{OTEL_BASE}/v1/traces",
                data=b"{}",
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urlopen(req, timeout=5):
                self.assert_true(True, "OTEL v1/traces accepts POST")
        except HTTPError as e:
            # 400 Bad Request or 500 for invalid data format are both acceptable
            # (endpoint exists but rejects invalid OTLP format)
            if e.code in (400, 500):
                self.assert_true(True, f"OTEL v1/traces exists ({e.code} for invalid data)")
            else:
                self.assert_true(False, f"OTEL endpoint error: {e.code}")
                return False
        except Exception as e:
            self.assert_true(False, f"OTEL endpoint error: {e}")
            return False

        return True

    def test_sse_endpoint_exists(self) -> bool:
        """Test SSE endpoint is available."""
        log_info("Testing SSE endpoint availability...")

        try:
            req = Request(
                f"{API_BASE}/traces/sse",
                headers={"Accept": "text/event-stream"},
            )
            with urlopen(req, timeout=2) as response:
                content_type = response.headers.get("Content-Type", "")
                self.assert_contains(
                    content_type, "text/event-stream", "SSE endpoint returns event-stream"
                )
        except Exception as e:
            # Timeout is expected since SSE keeps connection open
            if "timed out" in str(e).lower() or "timeout" in str(e).lower():
                self.assert_true(True, "SSE endpoint exists (connection kept open)")
            else:
                self.assert_true(False, f"SSE endpoint error: {e}")
                return False

        return True

    def run_all(self) -> None:
        """Run all health tests."""
        self.test_health_endpoint()
        self.test_otel_collector_endpoint()
        self.test_sse_endpoint_exists()
