"""Configuration constants for e2e tests."""

from pathlib import Path

# Paths
# __file__ = tests/e2e/src/config.py -> 4 parents to get to project root
PROJECT_ROOT = Path(__file__).parent.parent.parent.parent
SERVER_DIR = PROJECT_ROOT / "server"
DATA_DIR = PROJECT_ROOT / ".sideseat"

# Server
SERVER_HOST = "127.0.0.1"
SERVER_PORT = 5001
GRPC_PORT = 4317
BASE_URL = f"http://{SERVER_HOST}:{SERVER_PORT}"
API_BASE = f"{BASE_URL}/api/v1"
OTEL_BASE = f"{BASE_URL}/otel"

# Timeouts
SERVER_STARTUP_TIMEOUT = 90  # seconds (cargo build can be slow)
API_CALL_TIMEOUT = 15  # seconds
TRACE_PERSIST_WAIT = 3  # seconds to wait for traces to persist
