"""Server management for e2e tests."""

import os
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


def clean_data_dir() -> None:
    """Remove the data directory if it exists."""
    if DATA_DIR.exists():
        log_info(f"Removing existing data directory: {DATA_DIR}")
        shutil.rmtree(DATA_DIR)
        log_success("Data directory cleaned")
    else:
        log_info("No existing data directory found")


def start_server() -> subprocess.Popen | None:
    """Start the SideSeat server."""
    log_header("Starting SideSeat Server")

    env = os.environ.copy()
    env["SIDESEAT_DATA_DIR"] = str(DATA_DIR)
    env["SIDESEAT_SECRET_BACKEND"] = "file"
    env["SIDESEAT_AUTH_ENABLED"] = "false"
    env["RUST_LOG"] = "info,sideseat=debug"

    try:
        proc = subprocess.Popen(
            ["cargo", "run", "--", "start", "--no-auth"],
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


def wait_for_server(timeout: int = SERVER_STARTUP_TIMEOUT) -> bool:
    """Wait for the server to be ready."""
    log_info(f"Waiting for server at {SERVER_HOST}:{SERVER_PORT}...")
    start = time.time()
    last_log_time = 0

    while time.time() - start < timeout:
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
