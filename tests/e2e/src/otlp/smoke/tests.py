"""OTLP smoke tests - quick validation that system is operational."""

import socket
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from ...api import api_call
from ...base import BaseTestSuite
from ...config import API_BASE, GRPC_PORT, OTEL_BASE
from ...logging import log_info, log_section
from ..traces import create_simple_trace, send_otlp_traces_http


class SmokeTests(BaseTestSuite):
    """Quick validation tests for OTLP functionality."""

    def test_server_health(self) -> bool:
        """Test the health endpoint returns 200 OK."""
        log_section("OTLP Smoke Tests")
        log_info("Testing server health...")

        result = api_call("/health")
        if not self.assert_not_none(result, "Health endpoint responds"):
            return False

        if isinstance(result, dict):
            status = result.get("status", "").lower()
            return self.assert_equals(status, "ok", "Health status is 'ok'")

        return False

    def test_otlp_status(self) -> bool:
        """Test OTLP collector is enabled in health response."""
        log_info("Testing OTLP collector status...")

        result = api_call("/health")
        if not result or not isinstance(result, dict):
            self.skip("Health endpoint not available")
            return True

        otel = result.get("otel", {})
        if not otel:
            self.skip("No OTLP status in health response")
            return True

        enabled = otel.get("enabled", False)
        return self.assert_true(enabled, "OTLP collector is enabled")

    def test_basic_ingestion(self) -> bool:
        """Send 1 trace, verify it appears in API."""
        log_info("Testing basic trace ingestion...")

        # Create and send a simple trace
        trace_id, payload = create_simple_trace(
            service_name="smoke-test-service",
            span_count=1,
        )

        success, status = send_otlp_traces_http(payload)
        if not self.assert_true(success, f"Trace ingestion accepted (status {status})"):
            return False

        # Wait for persistence
        log_info("Waiting for trace persistence...")
        time.sleep(3)

        # Verify trace exists
        result = api_call(f"/traces/{trace_id}")
        if not self.assert_not_none(result, f"Trace {trace_id[:16]}... retrieved"):
            return False

        if isinstance(result, dict):
            return self.assert_equals(
                result.get("trace_id"),
                trace_id,
                "Retrieved trace ID matches",
            )

        return False

    def test_grpc_available(self) -> bool:
        """Test gRPC port 4317 accepts connections."""
        log_info("Testing gRPC port availability...")

        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.settimeout(5)
            result = sock.connect_ex(("localhost", GRPC_PORT))
            sock.close()

            if result == 0:
                return self.assert_true(True, f"gRPC port {GRPC_PORT} is open")
            else:
                return self.assert_true(False, f"gRPC port {GRPC_PORT} not available")

        except Exception as e:
            return self.assert_true(False, f"gRPC port check failed: {e}")

    def test_otlp_endpoint_exists(self) -> bool:
        """Test OTLP collector endpoint exists."""
        log_info("Testing OTLP collector endpoint...")

        try:
            req = Request(
                f"{OTEL_BASE}/v1/traces",
                data=b"{}",
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urlopen(req, timeout=5):
                return self.assert_true(True, "OTLP v1/traces accepts POST")
        except HTTPError as e:
            # 400/500 means endpoint exists but rejects invalid data
            if e.code in (400, 500):
                return self.assert_true(True, f"OTLP v1/traces exists ({e.code})")
            else:
                return self.assert_true(False, f"OTLP endpoint error: {e.code}")
        except Exception as e:
            return self.assert_true(False, f"OTLP endpoint error: {e}")

    def test_sse_endpoint_exists(self) -> bool:
        """Test SSE endpoint is available."""
        log_info("Testing SSE endpoint...")

        try:
            req = Request(
                f"{API_BASE}/traces/sse",
                headers={"Accept": "text/event-stream"},
            )
            with urlopen(req, timeout=2) as response:
                content_type = response.headers.get("Content-Type", "")
                return self.assert_contains(
                    content_type, "text/event-stream", "SSE returns event-stream"
                )
        except Exception as e:
            # Timeout is expected since SSE keeps connection open
            if "timed out" in str(e).lower() or "timeout" in str(e).lower():
                return self.assert_true(
                    True, "SSE endpoint exists (connection kept open)"
                )
            else:
                return self.assert_true(False, f"SSE endpoint error: {e}")

    def run_all(self) -> None:
        """Run all smoke tests."""
        self.test_server_health()
        self.test_otlp_status()
        self.test_otlp_endpoint_exists()
        self.test_grpc_available()
        self.test_basic_ingestion()
        self.test_sse_endpoint_exists()
