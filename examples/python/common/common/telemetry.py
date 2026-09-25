"""Common telemetry setup utilities.

Provides a base telemetry setup that can be customized with framework-specific
instrumentors. Supports both standard OpenTelemetry and SideSeat SDK modes.
"""

import os
from collections.abc import Callable
from contextlib import AbstractContextManager
from typing import Any

from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor, ConsoleSpanExporter
from opentelemetry.trace import Span


class NativeTraceClient:
    """Minimal SideSeat-compatible trace owner backed by a native OTel provider."""

    def __init__(self, provider: Any, instrumentation_scope: str) -> None:
        self.tracer_provider = provider
        self._tracer = provider.get_tracer(instrumentation_scope)

    def trace(
        self,
        name: str,
        *,
        session_id: str | None = None,
        user_id: str | None = None,
    ) -> AbstractContextManager[Span]:
        attributes = {}
        if session_id is not None:
            attributes["session.id"] = session_id
        if user_id is not None:
            attributes["user.id"] = user_id
        return self._tracer.start_as_current_span(name, attributes=attributes)

    def shutdown(self) -> None:
        shutdown = getattr(self.tracer_provider, "shutdown", None)
        if shutdown is not None:
            shutdown()


def setup_logfire_telemetry(
    instrument_method: str,
    service_name: str,
) -> NativeTraceClient:
    """Configure a provider SDK through Logfire without using the SideSeat SDK."""
    import logfire
    from sideseat.instrumentation import _suspend_otel_exporter_env

    # Logfire reads OTLP env during configure. Hide it until the native control
    # path attaches its one explicit exporter, then restore the application env.
    with _suspend_otel_exporter_env():
        logfire.configure(
            service_name=service_name,
            send_to_logfire=False,
            console=False,
        )
    getattr(logfire, instrument_method)()

    provider = trace.get_tracer_provider()
    if not hasattr(provider, "add_span_processor"):
        raise RuntimeError("Logfire did not create a usable TracerProvider")

    # Logfire exports a completed streaming response as a log span after its
    # request span has ended. Reattach that response before either native OTLP
    # exporter sees it, matching the topology SideSeat's SDK guarantees.
    from sideseat.telemetry.processors import _LogfireStreamingProcessor

    provider.add_span_processor(_LogfireStreamingProcessor())
    _add_standard_exporters(provider)
    return NativeTraceClient(provider, service_name)


def _add_standard_exporters(provider: Any) -> None:
    """Attach the console and SideSeat OTLP exporters to an existing provider."""
    provider.add_span_processor(BatchSpanProcessor(ConsoleSpanExporter()))
    if os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT"):
        provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
        return

    sideseat_base = os.getenv("SIDESEAT_ENDPOINT", "http://127.0.0.1:5388").rstrip("/")
    project_id = os.getenv("SIDESEAT_PROJECT_ID", "default")
    endpoint = f"{sideseat_base}/otel/{project_id}/v1/traces"
    provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter(endpoint=endpoint)))


def setup_base_telemetry(
    instrumentor: Callable[[], None] | None = None,
    use_sideseat: bool = False,
    framework: str | list[str] | None = None,
):
    """Initialize telemetry with standard configuration.

    Default: OpenTelemetry with console and OTLP exporters.
    Optional: SideSeat SDK with automatic OTLP setup + file exporter.

    Args:
        instrumentor: Optional callable that instruments the framework.
                      Should be a function that calls framework's instrumentor.
        use_sideseat: Use SideSeat SDK instead of default OpenTelemetry setup.
        framework: Framework/provider names for SideSeat (e.g.,
                   Frameworks.AutoGen or [Frameworks.LangGraph, Frameworks.Bedrock]).

    Returns:
        The telemetry provider/client instance.
    """
    if use_sideseat:
        from sideseat import SideSeat

        client = SideSeat(framework=framework)
        # client.telemetry.setup_file_exporter()
        client.telemetry.setup_console_exporter()

        # SideSeat owns framework instrumentation in this mode. Calling the sample's
        # native instrumentor again would patch the same framework twice and can emit
        # duplicate spans. The callback is only for the plain OpenTelemetry branch.
        return client
    else:
        provider = trace.get_tracer_provider()

        if not hasattr(provider, "add_span_processor"):
            provider = TracerProvider()
            trace.set_tracer_provider(provider)

        _add_standard_exporters(provider)

        if instrumentor:
            instrumentor(provider)

        return provider
