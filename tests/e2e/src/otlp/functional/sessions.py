"""OTLP Session tests - session aggregation and API endpoints."""

import time
import uuid

from ...api import api_call, encode_param
from ...base import BaseTestSuite
from ...config import TRACE_PERSIST_WAIT
from ...logging import log_info, log_section
from ..traces import create_simple_trace, send_otlp_traces_http


def generate_session_id() -> str:
    """Generate a random session ID."""
    return f"session-{uuid.uuid4().hex[:16]}"


class SessionTests(BaseTestSuite):
    """Session API endpoint tests."""

    def __init__(self) -> None:
        super().__init__()
        self.test_session_id: str = ""
        self.test_user_id: str = ""
        self.test_service: str = "session-test-service"
        self.trace_ids: list[str] = []

    def setup(self) -> None:
        """Create test data with sessions before running tests."""
        log_info("Setting up session test data...")

        self.test_session_id = generate_session_id()
        self.test_user_id = f"user-{uuid.uuid4().hex[:8]}"

        # Create multiple traces for the same session
        for i in range(3):
            trace_id, payload = create_simple_trace(
                service_name=self.test_service,
                span_count=3,
                with_genai=True,
                session_id=self.test_session_id,
                user_id=self.test_user_id,
            )
            success, _ = send_otlp_traces_http(payload)
            if success:
                self.trace_ids.append(trace_id)

        # Wait for persistence
        time.sleep(TRACE_PERSIST_WAIT)

    def test_session_list(self) -> bool:
        """Test GET /api/v1/sessions."""
        log_section("Session Tests")
        log_info("Testing session listing...")

        result = api_call("/sessions")
        if not self.assert_not_none(result, "Session listing returns data"):
            return False

        if isinstance(result, dict):
            sessions = result.get("sessions", [])
            self.assert_greater(len(sessions), 0, "At least one session exists")
            self.assert_true("has_more" in result, "Response has 'has_more'")
            self.assert_true("next_cursor" in result, "Response has 'next_cursor'")
            return True

        return False

    def test_session_not_found(self) -> bool:
        """Test GET /api/v1/sessions/{session_id} returns 404 for non-existent session."""
        log_info("Testing non-existent session returns 404...")

        fake_session_id = "non-existent-session-12345"
        result = api_call(
            f"/sessions/{encode_param(fake_session_id)}", expect_error=True
        )

        if result and isinstance(result, dict):
            return self.assert_true(
                result.get("code") == 404 or result.get("error"),
                "Non-existent session returns 404",
            )

        return self.assert_true(
            result is None, "Non-existent session request failed as expected"
        )

    def test_session_detail(self) -> bool:
        """Test GET /api/v1/sessions/{session_id} with all response fields."""
        log_info("Testing single session retrieval...")

        if not self.test_session_id:
            self.skip("No test session available")
            return True

        result = api_call(f"/sessions/{encode_param(self.test_session_id)}")
        if not self.assert_not_none(
            result, f"Session {self.test_session_id[:20]}... retrieved"
        ):
            return False

        if isinstance(result, dict):
            # Core identification
            self.assert_equals(
                result.get("session_id"),
                self.test_session_id,
                "Correct session returned",
            )
            self.assert_equals(
                result.get("user_id"),
                self.test_user_id,
                "User ID is correct",
            )
            self.assert_equals(
                result.get("service_name"),
                self.test_service,
                "Service name is correct",
            )

            # Aggregation counts
            self.assert_greater(
                result.get("trace_count", 0),
                0,
                "Session has trace count",
            )
            self.assert_greater(
                result.get("span_count", 0),
                0,
                "Session has span count",
            )

            # Timing fields
            self.assert_true(
                "first_seen_ns" in result,
                "Response has 'first_seen_ns'",
            )
            self.assert_true(
                "last_seen_ns" in result,
                "Response has 'last_seen_ns'",
            )
            self.assert_true(
                "duration_ns" in result,
                "Response has 'duration_ns'",
            )
            self.assert_greater_equal(
                result.get("duration_ns", -1),
                0,
                "Duration is non-negative",
            )

            # Error state (our test data has no errors - status_code=1 is OK)
            self.assert_true(
                "has_errors" in result,
                "Response has 'has_errors'",
            )
            self.assert_equals(
                result.get("has_errors"),
                False,
                "Session without errors has has_errors=false",
            )

            return True

        return False

    def test_session_aggregation(self) -> bool:
        """Test that session aggregates data from multiple traces."""
        log_info("Testing session aggregation...")

        if not self.test_session_id:
            self.skip("No test session available")
            return True

        result = api_call(f"/sessions/{encode_param(self.test_session_id)}")
        if not result or not isinstance(result, dict):
            return self.assert_true(False, "Could not retrieve session")

        # We created 3 traces with 3 spans each = 9 spans total
        trace_count = result.get("trace_count", 0)
        span_count = result.get("span_count", 0)

        self.assert_greater_equal(
            trace_count, 3, f"Trace count >= 3 (got {trace_count})"
        )
        self.assert_greater_equal(span_count, 9, f"Span count >= 9 (got {span_count})")

        # Token aggregation (100 input + 200 output per trace root span = 300 per trace)
        # 3 traces = 300 input tokens, 600 output tokens, 900 total
        total_input = result.get("total_input_tokens")
        total_output = result.get("total_output_tokens")
        total_tokens = result.get("total_tokens")

        if total_input is not None:
            self.assert_greater_equal(
                total_input, 300, f"Input tokens >= 300 (got {total_input})"
            )
        if total_output is not None:
            self.assert_greater_equal(
                total_output, 600, f"Output tokens >= 600 (got {total_output})"
            )
        if total_tokens is not None:
            self.assert_greater_equal(
                total_tokens, 900, f"Total tokens >= 900 (got {total_tokens})"
            )

        return True

    def test_session_traces(self) -> bool:
        """Test GET /api/v1/sessions/{session_id}/traces."""
        log_info("Testing session traces endpoint...")

        if not self.test_session_id:
            self.skip("No test session available")
            return True

        result = api_call(f"/sessions/{encode_param(self.test_session_id)}/traces")
        if not self.assert_not_none(result, "Session traces returns data"):
            return False

        if isinstance(result, dict):
            traces = result.get("traces", [])
            self.assert_greater(len(traces), 0, "Session has traces")
            self.assert_true("has_more" in result, "Response has 'has_more'")
            self.assert_true("next_cursor" in result, "Response has 'next_cursor'")

            # Verify trace response fields
            if traces:
                trace = traces[0]
                self.assert_true("trace_id" in trace, "Trace has 'trace_id'")
                self.assert_true(
                    "root_span_name" in trace, "Trace has 'root_span_name'"
                )
                self.assert_true("span_count" in trace, "Trace has 'span_count'")
                self.assert_true("start_time_ns" in trace, "Trace has 'start_time_ns'")
                self.assert_true("duration_ns" in trace, "Trace has 'duration_ns'")
                self.assert_true("has_errors" in trace, "Trace has 'has_errors'")

            # Verify our test traces are in the results
            found_test_traces = [
                t for t in traces if t.get("trace_id") in self.trace_ids
            ]
            self.assert_greater(
                len(found_test_traces),
                0,
                "Session traces contains our test traces",
            )

            return True

        return False

    def test_session_traces_pagination(self) -> bool:
        """Test cursor-based pagination for session traces."""
        log_info("Testing session traces pagination...")

        if not self.test_session_id:
            self.skip("No test session available")
            return True

        all_trace_ids: list[str] = []
        cursor = None
        page = 0
        max_pages = 3

        while page < max_pages:
            url = f"/sessions/{encode_param(self.test_session_id)}/traces?limit=2"
            if cursor:
                url += f"&cursor={encode_param(cursor)}"

            result = api_call(url)
            if not result or not isinstance(result, dict):
                break

            traces = result.get("traces", [])
            for t in traces:
                all_trace_ids.append(t["trace_id"])

            page += 1

            if not result.get("has_more"):
                break

            cursor = result.get("next_cursor")
            if not cursor:
                break

        if not all_trace_ids:
            self.skip("No traces available for pagination test")
            return True

        # Check for no duplicates
        unique_ids = set(all_trace_ids)
        return self.assert_equals(
            len(all_trace_ids),
            len(unique_ids),
            "No duplicate traces in session traces pagination",
        )

    def test_session_filters(self) -> bool:
        """Test session filtering by user_id and service."""
        log_info("Testing session filters...")

        # Filter by user_id - should find our test session
        result = api_call(f"/sessions?user_id={encode_param(self.test_user_id)}")
        if not self.assert_not_none(result, "user_id filter returns data"):
            return False

        if isinstance(result, dict):
            sessions = result.get("sessions", [])
            self.assert_greater(len(sessions), 0, "user_id filter returns sessions")
            all_match = all(s.get("user_id") == self.test_user_id for s in sessions)
            self.assert_true(all_match, "user_id filter returns only matching sessions")
            # Verify our test session is in the results
            found = any(s.get("session_id") == self.test_session_id for s in sessions)
            self.assert_true(found, "user_id filter finds our test session")

        # Filter by service - should find our test session
        result = api_call(f"/sessions?service={encode_param(self.test_service)}")
        if not self.assert_not_none(result, "service filter returns data"):
            return False

        if isinstance(result, dict):
            sessions = result.get("sessions", [])
            self.assert_greater(len(sessions), 0, "service filter returns sessions")
            all_match = all(
                s.get("service_name") == self.test_service for s in sessions
            )
            self.assert_true(all_match, "service filter returns only matching sessions")
            # Verify our test session is in the results
            found = any(s.get("session_id") == self.test_session_id for s in sessions)
            self.assert_true(found, "service filter finds our test session")

        return True

    def test_session_pagination(self) -> bool:
        """Test cursor-based pagination for sessions."""
        log_info("Testing session pagination...")

        all_session_ids: list[str] = []
        cursor = None
        page = 0
        max_pages = 3

        while page < max_pages:
            url = "/sessions?limit=2"
            if cursor:
                url += f"&cursor={encode_param(cursor)}"

            result = api_call(url)
            if not result or not isinstance(result, dict):
                break

            sessions = result.get("sessions", [])
            for s in sessions:
                all_session_ids.append(s["session_id"])

            page += 1

            if not result.get("has_more"):
                break

            cursor = result.get("next_cursor")
            if not cursor:
                break

        if not all_session_ids:
            self.skip("No sessions available for pagination test")
            return True

        # Check for no duplicates
        unique_ids = set(all_session_ids)
        return self.assert_equals(
            len(all_session_ids),
            len(unique_ids),
            "No duplicate sessions in pagination",
        )

    def test_session_error_state(self) -> bool:
        """Test that session reflects has_errors from error traces."""
        log_info("Testing session error state...")

        # Create a session with an error trace
        error_session_id = generate_session_id()
        trace_id, payload = create_simple_trace(
            service_name="error-session-service",
            span_count=1,
            session_id=error_session_id,
            with_error=True,
        )

        success, _ = send_otlp_traces_http(payload)
        if not success:
            self.skip("Could not create error trace for error state test")
            return True

        time.sleep(TRACE_PERSIST_WAIT)

        # Verify session has has_errors=true
        result = api_call(f"/sessions/{encode_param(error_session_id)}")
        if not result or not isinstance(result, dict):
            self.skip("Could not retrieve error session")
            return True

        self.assert_true(
            result.get("has_errors") is True,
            "Session with error trace has has_errors=true",
        )

        return True

    def test_session_delete(self) -> bool:
        """Test DELETE /api/v1/sessions/{session_id} - hard delete session and all traces."""
        log_info("Testing session deletion...")

        # Create a session specifically for deletion
        delete_session_id = generate_session_id()
        trace_id, payload = create_simple_trace(
            service_name="delete-session-service",
            span_count=1,
            session_id=delete_session_id,
        )

        success, _ = send_otlp_traces_http(payload)
        if not success:
            self.skip("Could not create session for deletion test")
            return True

        time.sleep(TRACE_PERSIST_WAIT)

        # Verify it exists
        result = api_call(f"/sessions/{encode_param(delete_session_id)}")
        if not result:
            self.skip("Session not found before deletion")
            return True

        # Delete it
        result = api_call(
            f"/sessions/{encode_param(delete_session_id)}", method="DELETE"
        )
        self.assert_true(
            result is not None, f"Delete request for {delete_session_id[:20]}..."
        )

        # Verify it's gone
        time.sleep(1)
        result = api_call(
            f"/sessions/{encode_param(delete_session_id)}", expect_error=True
        )
        if result and isinstance(result, dict):
            return self.assert_true(
                result.get("code") == 404 or result.get("error"),
                "Deleted session returns 404",
            )

        return self.assert_true(result is None, "Deleted session no longer accessible")

    def run_all(self) -> None:
        """Run all session tests."""
        self.setup()

        self.test_session_list()
        self.test_session_not_found()
        self.test_session_detail()
        self.test_session_aggregation()
        self.test_session_traces()
        self.test_session_traces_pagination()
        self.test_session_filters()
        self.test_session_pagination()
        self.test_session_error_state()
        self.test_session_delete()
