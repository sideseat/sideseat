"""Telemetry setup for BrowserUse captures."""

import os
from contextlib import AbstractContextManager
from typing import Any

from lmnr import Instruments, Laminar
from sideseat import Frameworks, SideSeat

SERVICE_NAME = "browser-use-sample"


class NativeLaminarTraceClient:
    """Minimal trace owner for BrowserUse's native Laminar path."""

    def __init__(self, saved_env: dict[str, str]) -> None:
        self._saved_env = saved_env

    def trace(
        self,
        name: str,
        *,
        session_id: str | None = None,
        user_id: str | None = None,
    ) -> AbstractContextManager[Any]:
        """Start a Laminar root span carrying SideSeat's session semantics."""
        return Laminar.start_as_current_span(
            name,
            session_id=session_id,
            user_id=user_id,
        )

    def shutdown(self) -> None:
        """Flush Laminar and restore exporter environment overrides."""
        try:
            Laminar.flush()
            Laminar.shutdown()
        finally:
            for key in (
                "LMNR_TRACE_CONTENT",
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
                "OTEL_SERVICE_NAME",
            ):
                os.environ.pop(key, None)
            os.environ.update(self._saved_env)


def _traces_endpoint() -> str:
    """Resolve a complete OTLP/HTTP traces endpoint for Laminar."""
    if endpoint := os.getenv("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"):
        return endpoint
    if endpoint := os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT"):
        return f"{endpoint.rstrip('/')}/v1/traces"
    base = os.getenv("SIDESEAT_ENDPOINT", "http://127.0.0.1:5388").rstrip("/")
    project_id = os.getenv("SIDESEAT_PROJECT_ID", "default")
    return f"{base}/otel/{project_id}/v1/traces"


def _native_telemetry() -> NativeLaminarTraceClient:
    """Initialize the same Laminar hooks BrowserUse uses without SideSeat."""
    for key in ("LMNR_PROJECT_API_KEY", "LMNR_BASE_URL"):
        if os.getenv(key):
            raise RuntimeError(f"unset {key}; native capture must export to SideSeat")

    keys = (
        "LMNR_TRACE_CONTENT",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
        "OTEL_SERVICE_NAME",
    )
    saved_env = {key: os.environ[key] for key in keys if key in os.environ}
    os.environ["LMNR_TRACE_CONTENT"] = "true"
    os.environ["OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"] = _traces_endpoint()
    os.environ["OTEL_EXPORTER_OTLP_TRACES_PROTOCOL"] = "http/protobuf"
    os.environ["OTEL_SERVICE_NAME"] = SERVICE_NAME
    try:
        Laminar.initialize(
            instruments={Instruments.OPENAI, Instruments.BUBUS},
            force_http=True,
            set_global_tracer_provider=True,
        )
    except Exception:
        for key in keys:
            os.environ.pop(key, None)
        os.environ.update(saved_env)
        raise
    return NativeLaminarTraceClient(saved_env)


def setup_telemetry(use_sideseat: bool = False) -> Any:
    """Initialize BrowserUse telemetry in native or SideSeat mode."""
    if use_sideseat:
        return SideSeat(
            framework=Frameworks.BrowserUse,
            service_name=SERVICE_NAME,
        )
    return _native_telemetry()
