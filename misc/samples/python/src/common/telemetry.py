"""Common telemetry setup utilities.

Provides a base telemetry setup that can be customized with framework-specific
instrumentors. Supports both standard OpenTelemetry and SideSeat SDK modes.
"""

import os
from typing import Callable

from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor, ConsoleSpanExporter


def setup_base_telemetry(
    instrumentor: Callable[[], None] | None = None,
    use_sideseat: bool = False,
    framework: str | None = None,
):
    """Initialize telemetry with standard configuration.

    Default: OpenTelemetry with console and OTLP exporters.
    Optional: SideSeat SDK with automatic OTLP setup + file exporter.

    Args:
        instrumentor: Optional callable that instruments the framework.
                      Should be a function that calls framework's instrumentor.
        use_sideseat: Use SideSeat SDK instead of default OpenTelemetry setup.
        framework: Framework name for SideSeat (e.g., Frameworks.AutoGen).

    Returns:
        The telemetry provider/client instance.
    """
    if use_sideseat:
        from sideseat import SideSeat

        client = SideSeat(framework=framework)
        client.telemetry.setup_file_exporter()
        client.telemetry.setup_console_exporter()

        if instrumentor:
            instrumentor()

        return client
    else:
        provider = trace.get_tracer_provider()

        if not hasattr(provider, "add_span_processor"):
            provider = TracerProvider()
            trace.set_tracer_provider(provider)

        provider.add_span_processor(BatchSpanProcessor(ConsoleSpanExporter()))

        if os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT"):
            provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))

        if instrumentor:
            instrumentor()

        return provider
