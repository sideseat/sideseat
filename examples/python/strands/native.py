"""Strands' documented telemetry setup: StrandsTelemetry on an OTLP-exporting provider."""

from strands.telemetry import StrandsTelemetry

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    StrandsTelemetry(tracer_provider=native.provider())
