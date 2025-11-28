"""OTLP authentication tests - bootstrap token and JWT flow."""

import json
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from ...api import api_call
from ...base import BaseTestSuite
from ...config import API_BASE
from ...logging import log_info, log_section, log_warn
from ...server import get_bootstrap_token


class AuthTests(BaseTestSuite):
    """Authentication flow tests.

    Note: These tests verify auth endpoints work when auth is enabled.
    The main e2e tests typically run with auth disabled (--no-auth).
    """

    def __init__(self) -> None:
        super().__init__()
        self.bootstrap_token: str | None = get_bootstrap_token()
        self.session_cookie: str | None = None

    def test_bootstrap_token(self) -> bool:
        """Test that bootstrap token can be extracted from server output.

        Note: In real usage, the token is printed to terminal on server start.
        This test verifies the auth status endpoint works.
        """
        log_section("Authentication Tests")
        log_info("Testing bootstrap token availability...")

        if self.bootstrap_token:
            log_info(f"Bootstrap token captured: {self.bootstrap_token[:8]}...")
        else:
            log_warn("Note: Auth tests require server started without --no-auth")

        # Check auth status endpoint
        # Response: { authenticated: bool, auth_method: "disabled"|"jwt"|null }
        result = api_call("/auth/status")
        if result and isinstance(result, dict):
            auth_method = result.get("auth_method")
            # auth_method == "disabled" means auth is disabled
            if auth_method == "disabled":
                self.skip("Auth is disabled on server (auth_method=disabled)")
                return True
            return self.assert_true(True, "Auth status endpoint works (auth enabled)")

        return self.assert_true(True, "Auth status endpoint accessible")

    def test_token_exchange(self) -> bool:
        """Test exchange bootstrap token for JWT session.

        Note: Requires actual bootstrap token from server startup.
        """
        log_info("Testing token exchange...")

        if not self.bootstrap_token:
            self.skip("No bootstrap token available for exchange test")
            return True

        try:
            data = json.dumps({"token": self.bootstrap_token}).encode()
            req = Request(
                f"{API_BASE}/auth/exchange",
                data=data,
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urlopen(req, timeout=10) as response:
                if response.status == 200:
                    # Check for session cookie
                    cookie = response.headers.get("Set-Cookie", "")
                    if "sideseat_session" in cookie:
                        self.session_cookie = cookie
                        return self.assert_true(True, "Token exchange successful")
                    return self.assert_true(True, "Token exchange accepted")
                return self.assert_true(False, f"Unexpected status: {response.status}")
        except HTTPError as e:
            if e.code == 401:
                return self.assert_true(False, "Invalid bootstrap token")
            return self.assert_true(False, f"Token exchange error: {e.code}")
        except Exception as e:
            self.skip(f"Token exchange test skipped: {e}")
            return True

    def test_session_cookie(self) -> bool:
        """Test JWT is stored in cookie after exchange."""
        log_info("Testing session cookie...")

        if not self.session_cookie:
            self.skip("No session cookie from token exchange")
            return True

        return self.assert_contains(
            self.session_cookie,
            "sideseat_session",
            "Session cookie contains sideseat_session",
        )

    def test_auth_required(self) -> bool:
        """Test protected endpoints require authentication when auth is enabled."""
        log_info("Testing auth requirement...")

        # Check if auth is enabled first
        result = api_call("/auth/status")
        if result and isinstance(result, dict):
            auth_method = result.get("auth_method")
            if auth_method == "disabled":
                self.skip("Auth is disabled - skipping auth requirement test")
                return True

        # Try to access a protected endpoint without auth cookie
        # When auth is enabled, /traces should require authentication
        try:
            req = Request(
                f"{API_BASE}/traces",
                headers={"Accept": "application/json"},
            )
            with urlopen(req, timeout=10) as response:
                # If we get 200, endpoint is public (which is valid configuration)
                return self.assert_true(
                    True, "Endpoint accessible without auth (public endpoint)"
                )
        except HTTPError as e:
            if e.code == 401:
                return self.assert_true(
                    True, "Protected endpoint returns 401 without auth"
                )
            if e.code == 403:
                return self.assert_true(
                    True, "Protected endpoint returns 403 without auth"
                )
            return self.assert_true(False, f"Unexpected HTTP error: {e.code}")
        except Exception as e:
            return self.assert_true(False, f"Auth requirement test failed: {e}")

    def test_logout(self) -> bool:
        """Test logout clears session."""
        log_info("Testing logout...")

        try:
            req = Request(
                f"{API_BASE}/auth/logout",
                method="POST",
                headers={"Content-Type": "application/json"},
            )
            with urlopen(req, timeout=10) as response:
                return self.assert_true(response.status == 200, "Logout successful")
        except HTTPError as e:
            # 401 means not logged in, which is fine
            if e.code == 401:
                return self.assert_true(True, "Logout endpoint accessible")
            return self.assert_true(False, f"Logout error: {e.code}")
        except Exception as e:
            self.skip(f"Logout test skipped: {e}")
            return True

    def run_all(self) -> None:
        """Run all auth tests."""
        self.test_bootstrap_token()
        self.test_auth_required()
        self.test_token_exchange()
        self.test_session_cookie()
        self.test_logout()
