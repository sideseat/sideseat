"""SSE (Server-Sent Events) tests - real-time event streaming."""

import json
import queue
import threading
import time
import uuid
from typing import Any
from urllib.request import Request, urlopen  # urlopen only used for OTLP ingestion

from ...config import API_BASE, OTEL_BASE
from ...logging import log_info, log_section, log_success, log_warn
from ..base import BaseTestSuite


# Latency thresholds
MAX_EVENT_LATENCY_MS = 500  # Events should arrive within 500ms of ingestion


def generate_trace_id() -> str:
    """Generate a random 32-character hex trace ID."""
    return uuid.uuid4().hex + uuid.uuid4().hex[:16]


def generate_span_id() -> str:
    """Generate a random 16-character hex span ID."""
    return uuid.uuid4().hex[:16]


class SSEClient:
    """Simple SSE client for testing using http.client for proper streaming."""

    def __init__(self, url: str, timeout: float = 30.0):
        self.url = url
        self.timeout = timeout
        self.events: queue.Queue[dict[str, Any]] = queue.Queue()
        self.running = False
        self.thread: threading.Thread | None = None
        self.error: Exception | None = None
        self.connected = threading.Event()
        self._conn: Any = None

    def start(self) -> None:
        """Start receiving SSE events in background thread."""
        self.running = True
        self.thread = threading.Thread(target=self._receive_events, daemon=True)
        self.thread.start()

    def stop(self) -> None:
        """Stop receiving events."""
        self.running = False
        # Close the connection to unblock reads
        if self._conn:
            try:
                self._conn.close()
            except Exception:
                pass
        if self.thread:
            self.thread.join(timeout=2.0)

    def wait_for_connection(self, timeout: float = 5.0) -> bool:
        """Wait for connection to be established."""
        return self.connected.wait(timeout=timeout)

    def get_event(self, timeout: float = 5.0) -> dict[str, Any] | None:
        """Get next event from queue."""
        try:
            return self.events.get(timeout=timeout)
        except queue.Empty:
            return None

    def get_all_events(self, timeout: float = 0.1) -> list[dict[str, Any]]:
        """Get all pending events."""
        events = []
        while True:
            try:
                events.append(self.events.get(timeout=timeout))
            except queue.Empty:
                break
        return events

    def _receive_events(self) -> None:
        """Receive SSE events from the server using http.client for streaming."""
        import http.client
        import socket
        from urllib.parse import urlparse

        try:
            parsed = urlparse(self.url)
            host = parsed.hostname or "localhost"
            port = parsed.port or 80
            path = parsed.path
            if parsed.query:
                path = f"{path}?{parsed.query}"

            self._conn = http.client.HTTPConnection(host, port, timeout=self.timeout)
            self._conn.request(
                "GET",
                path,
                headers={
                    "Accept": "text/event-stream",
                    "Cache-Control": "no-cache",
                    "Connection": "keep-alive",
                },
            )
            response = self._conn.getresponse()

            if response.status != 200:
                self.error = Exception(f"HTTP {response.status}: {response.reason}")
                self.connected.set()
                return

            # Set socket timeout for reads (0.5s) to allow checking self.running
            sock = self._conn.sock
            if sock:
                sock.settimeout(0.5)

            self.connected.set()
            event_lines: list[str] = []

            while self.running:
                try:
                    # Read line by line (SSE is line-based protocol)
                    line_bytes = response.readline()

                    # readline() returns b"" on EOF, but may also return b""
                    # on timeout with chunked encoding. Check if connection is alive.
                    if not line_bytes:
                        # Only break if we're explicitly stopped
                        if not self.running:
                            break
                        # On chunked encoding timeout, readline() may return empty
                        # Continue to retry
                        continue

                    line = line_bytes.decode("utf-8").rstrip("\r\n")

                    # Empty line signals end of event
                    if line == "":
                        if event_lines:
                            event = self._parse_event("\n".join(event_lines))
                            if event:
                                self.events.put(event)
                            event_lines = []
                    else:
                        event_lines.append(line)

                except socket.timeout:
                    # Socket timeout, check if we should continue
                    continue
                except OSError as e:
                    # Connection error (including broken pipe, reset, etc.)
                    if self.running:
                        time.sleep(0.01)
                        continue
                    break
                except Exception as e:
                    if self.running:
                        # Other error, try to continue
                        time.sleep(0.01)
                        continue
                    break
        except Exception as e:
            self.error = e
            self.connected.set()  # Unblock waiters even on error
        finally:
            if self._conn:
                try:
                    self._conn.close()
                except Exception:
                    pass

    def _parse_event(self, event_str: str) -> dict[str, Any] | None:
        """Parse an SSE event string."""
        data = None
        event_type = None

        for line in event_str.split("\n"):
            if line.startswith("data:"):
                data = line[5:].strip()
            elif line.startswith("event:"):
                event_type = line[6:].strip()
            elif line.startswith(":"):
                # Comment (keepalive), skip
                continue

        if data:
            try:
                parsed = json.loads(data)
                if event_type:
                    parsed["_event_type"] = event_type
                parsed["_received_at"] = time.time() * 1000  # ms timestamp
                return parsed
            except json.JSONDecodeError:
                return None
        return None


class SSETests(BaseTestSuite):
    """SSE (Server-Sent Events) tests for real-time event streaming."""

    def __init__(self) -> None:
        super().__init__()
        self.sse_base = f"{API_BASE}/traces/sse"
        self.service_name = "sse-test-service"

    def _create_span(
        self,
        trace_id: str,
        span_id: str,
        parent_span_id: str | None,
        name: str,
        service_name: str,
        framework: str = "test-framework",
        has_error: bool = False,
    ) -> dict[str, Any]:
        """Create a span for OTLP ingestion."""
        now_ns = int(time.time() * 1_000_000_000)
        duration_ns = 100_000_000  # 100ms

        span = {
            "traceId": trace_id,
            "spanId": span_id,
            "name": name,
            "kind": 1,
            "startTimeUnixNano": str(now_ns),
            "endTimeUnixNano": str(now_ns + duration_ns),
            "attributes": [
                {"key": "test.sse", "value": {"boolValue": True}},
            ],
            # OTEL status: 0=UNSET, 1=OK, 2=ERROR
            # Server treats status_code != 0 as error, so use 0 for success
            "status": {"code": 2 if has_error else 0},
        }

        if parent_span_id:
            span["parentSpanId"] = parent_span_id

        return span

    def _create_otlp_payload(
        self, spans: list[dict[str, Any]], service_name: str, framework: str = "test-framework"
    ) -> dict[str, Any]:
        """Create an OTLP ExportTraceServiceRequest payload."""
        return {
            "resourceSpans": [
                {
                    "resource": {
                        "attributes": [
                            {"key": "service.name", "value": {"stringValue": service_name}},
                            {"key": "telemetry.sdk.name", "value": {"stringValue": framework}},
                        ]
                    },
                    "scopeSpans": [
                        {
                            "scope": {"name": "sse-test", "version": "1.0.0"},
                            "spans": spans,
                        }
                    ],
                }
            ]
        }

    def _send_trace(self, payload: dict[str, Any]) -> bool:
        """Send OTLP trace to the collector."""
        data = json.dumps(payload).encode("utf-8")
        try:
            req = Request(
                f"{OTEL_BASE}/v1/traces",
                data=data,
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urlopen(req, timeout=10) as response:
                return response.status == 200
        except Exception as e:
            log_warn(f"Failed to send trace: {e}")
            return False

    def test_sse_connection(self) -> bool:
        """Test SSE connection can be established."""
        log_section("SSE Tests - Connection")
        log_info("Testing SSE connection establishment...")

        client = SSEClient(self.sse_base)
        client.start()

        try:
            connected = client.wait_for_connection(timeout=5.0)
            if client.error:
                self.assert_true(False, f"SSE connection failed: {client.error}")
                return False

            self.assert_true(connected, "SSE connection established")
            return connected
        finally:
            client.stop()

    def test_sse_receives_events(self) -> bool:
        """Test that SSE receives events when traces are ingested."""
        log_info("Testing SSE receives events on ingestion...")

        client = SSEClient(self.sse_base)
        client.start()

        try:
            if not client.wait_for_connection(timeout=5.0):
                self.assert_true(False, "SSE connection timeout")
                return False

            # Give the connection a moment to stabilize
            time.sleep(0.2)

            # Send a trace
            trace_id = generate_trace_id()
            span_id = generate_span_id()
            span = self._create_span(
                trace_id=trace_id,
                span_id=span_id,
                parent_span_id=None,
                name="sse-test-span",
                service_name=self.service_name,
            )
            payload = self._create_otlp_payload([span], self.service_name)

            send_time = time.time() * 1000  # ms
            if not self._send_trace(payload):
                self.assert_true(False, "Failed to send trace")
                return False

            # Wait for the event
            event = client.get_event(timeout=5.0)

            if event is None:
                self.assert_true(False, "No SSE event received within timeout")
                return False

            # Verify event content
            event_data = event.get("event", {})
            event_type = event_data.get("type")

            self.assert_true(event_type == "NewSpan", f"Event type is NewSpan: {event_type}")

            span_data = event_data.get("data", {})
            self.assert_true(
                span_data.get("trace_id") == trace_id,
                f"Event contains correct trace_id",
            )
            self.assert_true(
                span_data.get("span_id") == span_id,
                f"Event contains correct span_id",
            )

            return True
        finally:
            client.stop()

    def test_sse_event_latency(self) -> bool:
        """Test that SSE events are received with low latency."""
        log_section("SSE Tests - Latency")
        log_info(f"Testing SSE event latency (threshold: {MAX_EVENT_LATENCY_MS}ms)...")

        client = SSEClient(self.sse_base)
        client.start()

        try:
            if not client.wait_for_connection(timeout=5.0):
                self.assert_true(False, "SSE connection timeout")
                return False

            time.sleep(0.2)

            latencies = []
            num_tests = 5

            for i in range(num_tests):
                trace_id = generate_trace_id()
                span_id = generate_span_id()
                span = self._create_span(
                    trace_id=trace_id,
                    span_id=span_id,
                    parent_span_id=None,
                    name=f"latency-test-{i}",
                    service_name=self.service_name,
                )
                payload = self._create_otlp_payload([span], self.service_name)

                send_time = time.time() * 1000

                if not self._send_trace(payload):
                    continue

                # Wait for event with matching trace_id
                start_wait = time.time()
                while time.time() - start_wait < 5.0:
                    event = client.get_event(timeout=1.0)
                    if event:
                        event_data = event.get("event", {})
                        if event_data.get("type") == "NewSpan":
                            span_data = event_data.get("data", {})
                            if span_data.get("trace_id") == trace_id:
                                receive_time = event.get("_received_at", time.time() * 1000)
                                latency = receive_time - send_time
                                latencies.append(latency)
                                break

                time.sleep(0.1)  # Small gap between tests

            if not latencies:
                self.assert_true(False, "No latency measurements collected")
                return False

            avg_latency = sum(latencies) / len(latencies)
            max_latency = max(latencies)
            min_latency = min(latencies)

            log_success(
                f"SSE latency: avg={avg_latency:.1f}ms, min={min_latency:.1f}ms, max={max_latency:.1f}ms"
            )

            passed = avg_latency < MAX_EVENT_LATENCY_MS
            self.assert_true(
                passed, f"Average latency < {MAX_EVENT_LATENCY_MS}ms: {avg_latency:.1f}ms"
            )
            return passed

        finally:
            client.stop()

    def test_sse_filter_by_service(self) -> bool:
        """Test SSE filtering by service name."""
        log_section("SSE Tests - Filtering")
        log_info("Testing SSE filter by service...")

        filter_service = "sse-filtered-service"
        other_service = "sse-other-service"

        # Connect with service filter
        filtered_url = f"{self.sse_base}?service={filter_service}"
        client = SSEClient(filtered_url)
        client.start()

        try:
            if not client.wait_for_connection(timeout=5.0):
                self.assert_true(False, "SSE connection timeout")
                return False

            time.sleep(0.2)

            # Send trace for OTHER service (should NOT be received)
            other_trace_id = generate_trace_id()
            other_span = self._create_span(
                trace_id=other_trace_id,
                span_id=generate_span_id(),
                parent_span_id=None,
                name="other-service-span",
                service_name=other_service,
            )
            self._send_trace(self._create_otlp_payload([other_span], other_service))

            # Send trace for FILTERED service (should be received)
            filtered_trace_id = generate_trace_id()
            filtered_span = self._create_span(
                trace_id=filtered_trace_id,
                span_id=generate_span_id(),
                parent_span_id=None,
                name="filtered-service-span",
                service_name=filter_service,
            )
            self._send_trace(self._create_otlp_payload([filtered_span], filter_service))

            # Wait and collect events
            time.sleep(1.0)
            events = client.get_all_events(timeout=0.5)

            # Check that we only received the filtered service's events
            received_trace_ids = set()
            for event in events:
                event_data = event.get("event", {})
                if event_data.get("type") == "NewSpan":
                    span_data = event_data.get("data", {})
                    received_trace_ids.add(span_data.get("trace_id"))

            # Should have received the filtered trace
            has_filtered = filtered_trace_id in received_trace_ids
            # Should NOT have received the other trace
            has_other = other_trace_id in received_trace_ids

            self.assert_true(has_filtered, "Received event from filtered service")
            self.assert_true(not has_other, "Did NOT receive event from other service")

            return has_filtered and not has_other

        finally:
            client.stop()

    def test_sse_filter_errors_only(self) -> bool:
        """Test SSE filtering for errors only."""
        log_info("Testing SSE filter errors_only...")

        filtered_url = f"{self.sse_base}?errors_only=true"
        client = SSEClient(filtered_url)
        client.start()

        try:
            if not client.wait_for_connection(timeout=5.0):
                self.assert_true(False, "SSE connection timeout")
                return False

            time.sleep(0.2)

            # Send SUCCESS trace (should NOT be received)
            success_trace_id = generate_trace_id()
            success_span = self._create_span(
                trace_id=success_trace_id,
                span_id=generate_span_id(),
                parent_span_id=None,
                name="success-span",
                service_name=self.service_name,
                has_error=False,
            )
            self._send_trace(self._create_otlp_payload([success_span], self.service_name))

            # Send ERROR trace (should be received)
            error_trace_id = generate_trace_id()
            error_span = self._create_span(
                trace_id=error_trace_id,
                span_id=generate_span_id(),
                parent_span_id=None,
                name="error-span",
                service_name=self.service_name,
                has_error=True,
            )
            self._send_trace(self._create_otlp_payload([error_span], self.service_name))

            time.sleep(1.0)
            events = client.get_all_events(timeout=0.5)

            received_trace_ids = set()
            for event in events:
                event_data = event.get("event", {})
                if event_data.get("type") == "NewSpan":
                    span_data = event_data.get("data", {})
                    received_trace_ids.add(span_data.get("trace_id"))

            has_error = error_trace_id in received_trace_ids
            has_success = success_trace_id in received_trace_ids

            self.assert_true(has_error, "Received error event")
            self.assert_true(not has_success, "Did NOT receive success event")

            return has_error and not has_success

        finally:
            client.stop()

    def test_sse_multiple_connections(self) -> bool:
        """Test multiple concurrent SSE connections."""
        log_section("SSE Tests - Concurrency")
        log_info("Testing multiple concurrent SSE connections...")

        num_connections = 5
        clients: list[SSEClient] = []

        try:
            # Create multiple connections
            for i in range(num_connections):
                client = SSEClient(self.sse_base)
                client.start()
                clients.append(client)

            # Wait for all to connect
            all_connected = True
            for i, client in enumerate(clients):
                if not client.wait_for_connection(timeout=5.0):
                    log_warn(f"Client {i} failed to connect")
                    all_connected = False

            self.assert_true(all_connected, f"All {num_connections} SSE connections established")

            if not all_connected:
                return False

            time.sleep(0.2)

            # Send a trace
            trace_id = generate_trace_id()
            span = self._create_span(
                trace_id=trace_id,
                span_id=generate_span_id(),
                parent_span_id=None,
                name="broadcast-test-span",
                service_name=self.service_name,
            )
            self._send_trace(self._create_otlp_payload([span], self.service_name))

            time.sleep(1.0)

            # Check all clients received the event
            clients_received = 0
            for i, client in enumerate(clients):
                events = client.get_all_events(timeout=0.5)
                for event in events:
                    event_data = event.get("event", {})
                    if event_data.get("type") == "NewSpan":
                        span_data = event_data.get("data", {})
                        if span_data.get("trace_id") == trace_id:
                            clients_received += 1
                            break

            self.assert_true(
                clients_received == num_connections,
                f"All {num_connections} clients received event: {clients_received}/{num_connections}",
            )

            return clients_received == num_connections

        finally:
            for client in clients:
                client.stop()

    def run_all(self) -> None:
        """Run all SSE tests."""
        self.test_sse_connection()
        self.test_sse_receives_events()
        self.test_sse_event_latency()
        self.test_sse_filter_by_service()
        self.test_sse_filter_errors_only()
        self.test_sse_multiple_connections()
