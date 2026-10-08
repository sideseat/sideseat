"""Plain OpenTelemetry, as the GenAI utilities' documentation sets it up: a global tracer provider.

The utilities create their spans from the global provider, so installing one with an OTLP exporter is
the whole setup; what they record, and with how much content, is the application's own configuration
(see ``tools.py``).
"""

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    native.provider()
