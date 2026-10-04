"""TraceLoop's documented setup for a backend of your own: ``Traceloop.init`` with an exporter."""

from traceloop.sdk import Traceloop
from traceloop.sdk.instruments import Instruments

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    Traceloop.init(
        app_name=native.service_name,
        exporter=native.exporter(),
        # The HTTP client instrumentations would trace the exporter's own requests.
        block_instruments={Instruments.REQUESTS, Instruments.URLLIB3},
        telemetry_enabled=False,
    )
