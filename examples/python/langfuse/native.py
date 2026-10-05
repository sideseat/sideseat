"""Langfuse's documented setup beside an existing OpenTelemetry pipeline.

Langfuse 4 is built on OpenTelemetry: given a tracer provider it records its observations there. A
user who sends traces to their own backend passes the application's provider, which already exports
over OTLP, and gives Langfuse an exporter that sends nothing to Langfuse's cloud.
"""

import os
from collections.abc import Sequence

from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.sdk.trace.export import SpanExporter, SpanExportResult

from harness.telemetry import NativeTelemetry


class _NoCloudExport(SpanExporter):
    def export(self, spans: Sequence[ReadableSpan]) -> SpanExportResult:
        return SpanExportResult.SUCCESS


def configure(native: NativeTelemetry) -> None:
    from langfuse import Langfuse

    # Langfuse replaces inline images and documents with references and uploads the bytes to its
    # cloud. With upload off it leaves them in the observation, where any OTLP backend can read them.
    os.environ["LANGFUSE_MEDIA_UPLOAD_ENABLED"] = "false"

    Langfuse(
        public_key="pk-lf-local",
        secret_key="sk-lf-local",
        tracer_provider=native.provider(),
        span_exporter=_NoCloudExport(),
    )
