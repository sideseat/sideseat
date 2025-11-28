"""Server management for e2e tests."""

import os
import re
import shutil
import subprocess
import time
from urllib.request import Request, urlopen

from .config import (
    DATA_DIR,
    SERVER_DIR,
    SERVER_HOST,
    SERVER_PORT,
    SERVER_STARTUP_TIMEOUT,
)
from .logging import log_error, log_header, log_info, log_success, log_warn

# Global to store bootstrap token when auth is enabled
_bootstrap_token: str | None = None


def get_bootstrap_token() -> str | None:
    """Get the bootstrap token captured during server startup."""
    return _bootstrap_token


def clean_data_dir() -> None:
    """Remove the data directory if it exists."""
    if DATA_DIR.exists():
        log_info(f"Removing existing data directory: {DATA_DIR}")
        shutil.rmtree(DATA_DIR)
        log_success("Data directory cleaned")
    else:
        log_info("No existing data directory found")


def start_server(no_auth: bool = False) -> subprocess.Popen | None:
    """Start the SideSeat server.

    Args:
        no_auth: If True, start server with authentication disabled (--no-auth).
    """
    global _bootstrap_token
    _bootstrap_token = None

    log_header("Starting SideSeat Server")

    env = os.environ.copy()
    env["SIDESEAT_DATA_DIR"] = str(DATA_DIR)
    env["SIDESEAT_SECRET_BACKEND"] = "file"
    env["RUST_LOG"] = "info,sideseat=debug"

    if no_auth:
        env["SIDESEAT_AUTH_ENABLED"] = "false"
        cmd = ["cargo", "run", "--", "start", "--no-auth"]
        log_info("Starting with authentication DISABLED (--no-auth)")
    else:
        env["SIDESEAT_AUTH_ENABLED"] = "true"
        cmd = ["cargo", "run", "--", "start"]
        log_info("Starting with authentication ENABLED")

    try:
        proc = subprocess.Popen(
            cmd,
            cwd=SERVER_DIR,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        log_success(f"Server process started (PID: {proc.pid})")
        return proc
    except Exception as e:
        log_error(f"Failed to start server: {e}")
        return None


def wait_for_server(
    timeout: int = SERVER_STARTUP_TIMEOUT,
    server_proc: subprocess.Popen | None = None,
) -> bool:
    """Wait for the server to be ready.

    Args:
        timeout: Maximum time to wait for server.
        server_proc: Server process to read output from (for capturing bootstrap token).
    """
    global _bootstrap_token
    log_info(f"Waiting for server at {SERVER_HOST}:{SERVER_PORT}...")
    start = time.time()
    last_log_time = 0

    while time.time() - start < timeout:
        # Try to read server output for bootstrap token
        if server_proc and server_proc.stdout and not _bootstrap_token:
            try:
                import select

                if hasattr(select, "select"):
                    # Non-blocking read on Unix
                    import fcntl

                    fd = server_proc.stdout.fileno()
                    fl = fcntl.fcntl(fd, fcntl.F_GETFL)
                    fcntl.fcntl(fd, fcntl.F_SETFL, fl | os.O_NONBLOCK)
                    try:
                        # Read all available lines to find the token
                        while True:
                            line = server_proc.stdout.readline()
                            if not line:
                                break
                            # Look for bootstrap token in URL: /ui?token=TOKEN
                            token_match = re.search(r"\?token=([a-fA-F0-9]+)", line)
                            if token_match:
                                _bootstrap_token = token_match.group(1)
                                log_success(
                                    f"Captured bootstrap token: {_bootstrap_token[:8]}..."
                                )
                                break
                    except (IOError, BlockingIOError):
                        pass
                    finally:
                        fcntl.fcntl(fd, fcntl.F_SETFL, fl)
            except Exception:
                pass

        try:
            req = Request(f"http://{SERVER_HOST}:{SERVER_PORT}/api/v1/health")
            with urlopen(req, timeout=2) as response:
                if response.status == 200:
                    elapsed = time.time() - start
                    log_success(f"Server ready in {elapsed:.1f}s")
                    return True
        except Exception:
            pass

        time.sleep(1)

        # Print progress every 10 seconds
        elapsed = time.time() - start
        current_10s = int(elapsed) // 10
        if current_10s > last_log_time:
            last_log_time = current_10s
            log_info(f"Still waiting... ({int(elapsed)}s)")

    log_error(f"Server failed to start within {timeout}s")
    return False


def cleanup_server(server_proc: subprocess.Popen | None) -> None:
    """Clean up server process."""
    log_header("Cleanup")

    if server_proc:
        log_info(f"Stopping server (PID: {server_proc.pid})...")
        try:
            server_proc.terminate()
            server_proc.wait(timeout=10)
            log_success("Server stopped gracefully")
        except subprocess.TimeoutExpired:
            log_warn("Server didn't stop gracefully, killing...")
            server_proc.kill()
            server_proc.wait()
            log_success("Server killed")
        except Exception as e:
            log_error(f"Error stopping server: {e}")
