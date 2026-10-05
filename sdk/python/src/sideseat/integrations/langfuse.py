"""Langfuse: records its observations on SideSeat's tracer provider instead of exporting them."""

from __future__ import annotations

from collections.abc import Sequence
from typing import Any

from opentelemetry.sdk.trace import ReadableSpan
from opentelemetry.sdk.trace.export import SpanExporter, SpanExportResult

from sideseat.integrations._base import Integration, SetupContext
from sideseat.integrations._util import temporary_env


class Langfuse(Integration):
    name = "langfuse"
    packages = ("langfuse",)
    extra = "langfuse"

    def instrument(self, ctx: SetupContext) -> None:
        from langfuse import Langfuse as Client

        # Given a provider, Langfuse adds its own processor to it - the one that stamps propagated
        # session, user and trace attributes on each span - and exports through the exporter it is
        # handed. SideSeat's pipeline already exports, so Langfuse's exporter sends nothing.
        # Media upload would replace inline images and documents with references to Langfuse's
        # cloud and send the bytes there; off, they stay in the observation SideSeat exports.
        with temporary_env({"LANGFUSE_MEDIA_UPLOAD_ENABLED": "false"}):
            self._client: Any = Client(
                public_key="pk-lf-sideseat",
                secret_key="sk-lf-sideseat",
                tracer_provider=ctx.tracer_provider,
                span_exporter=_NoExport(),
            )

    def flush(self, timeout_millis: int) -> bool:
        client = getattr(self, "_client", None)
        if client is not None:
            client.flush()
        return True


class _NoExport(SpanExporter):
    def export(self, spans: Sequence[ReadableSpan]) -> SpanExportResult:
        return SpanExportResult.SUCCESS
