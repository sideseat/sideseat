"""Every test starts with no SideSeat client, no OpenTelemetry globals, and a clean environment."""

from collections.abc import Iterator

import pytest

import sideseat
from sideseat.testing import reset_global_providers

_ENV = (
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    "OTEL_EXPORTER_OTLP_HEADERS",
    "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
    "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
    "OTEL_SERVICE_NAME",
    "OTEL_SERVICE_VERSION",
    "OTEL_RESOURCE_ATTRIBUTES",
    "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT",
    "LMNR_TRACE_CONTENT",
    "LMNR_PROJECT_API_KEY",
    "LMNR_BASE_URL",
    "SIDESEAT_ENDPOINT",
    "SIDESEAT_API_KEY",
    "SIDESEAT_PROJECT_ID",
    "SIDESEAT_INTEGRATIONS",
    "SIDESEAT_CAPTURE_CONTENT",
    "SIDESEAT_DISABLED",
    "SIDESEAT_DEBUG",
)


@pytest.fixture(autouse=True)
def isolated(monkeypatch: pytest.MonkeyPatch) -> Iterator[None]:
    for name in _ENV:
        monkeypatch.delenv(name, raising=False)
    reset_global_providers()
    yield
    sideseat.shutdown()
    reset_global_providers()
