"""OpenInference's documented setup: an OTLP-exporting tracer provider, installed globally.

The OpenInference instrumentors and the ``OITracer`` decorators then trace onto that provider; the
application installs them in ``models.py``, the same way in both modes.
"""

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    native.provider()
