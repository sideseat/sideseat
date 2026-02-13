"""Test fixtures with proper OpenTelemetry cleanup.

Based on patterns from strands-agents/sdk-python.
"""

from collections.abc import Iterator
from pathlib import Path

import pytest


@pytest.fixture(autouse=True)
def clean_otel_env(monkeypatch: pytest.MonkeyPatch) -> None:
    """Remove OpenTelemetry and SideSeat environment variables to prevent test pollution.

    This follows the pattern from strands-agents to ensure tests don't
    accidentally send telemetry to external endpoints.
    """
    otel_env_vars = [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_HEADERS",
        "OTEL_SERVICE_NAME",
        "OTEL_RESOURCE_ATTRIBUTES",
        "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT",
        "OTEL_PYTHON_LOGGING_AUTO_INSTRUMENTATION_ENABLED",
    ]
    sideseat_env_vars = [
        "SIDESEAT_ENDPOINT",
        "SIDESEAT_API_KEY",
        "SIDESEAT_PROJECT",
        "SIDESEAT_DISABLED",
        "SIDESEAT_DEBUG",
    ]
    for var in otel_env_vars + sideseat_env_vars:
        monkeypatch.delenv(var, raising=False)


@pytest.fixture(autouse=True)
def reset_global_instance() -> Iterator[None]:
    """Reset global SideSeat instance between tests."""
    import sideseat

    yield
    # Cleanup after test
    if sideseat.is_initialized():
        sideseat.shutdown()


@pytest.fixture
def temp_trace_file(tmp_path: Path) -> str:
    """Provide a temporary file path for trace output."""
    return str(tmp_path / "traces.jsonl")
