"""TraceLoop (OpenLLMetry): enriches spans on SideSeat's provider without exporting them itself."""

from __future__ import annotations

from typing import Any

from opentelemetry.sdk.trace import ReadableSpan, Span, SpanProcessor

from sideseat.integrations._base import Integration, SetupContext
from sideseat.integrations._util import content_switch


class TraceLoop(Integration):
    name = "traceloop"
    packages = ("traceloop-sdk",)
    extra = "traceloop"

    def instrument(self, ctx: SetupContext) -> None:
        from traceloop.sdk import Traceloop
        from traceloop.sdk.instruments import Instruments

        # TraceLoop snapshots its content setting at init, and its instrumentations read the same
        # variable again on every call.
        content_switch(ctx.settings, "TRACELOOP_TRACE_CONTENT")
        Traceloop.init(
            app_name=ctx.service_name,
            # Given an exporter, TraceLoop adds a second export pipeline; given a processor it
            # wraps that processor's on_start with its workflow enrichment and exports nothing.
            processor=_EnrichmentOnly(),
            resource_attributes=dict(ctx.resource.attributes),
            # Instrumenting requests or urllib3 would trace SideSeat's own OTLP exports.
            block_instruments={Instruments.REQUESTS, Instruments.URLLIB3},
            image_uploader=_InlineImages(),
            use_attributes=True,
        )


class _EnrichmentOnly(SpanProcessor):
    def on_start(self, span: Span, parent_context: Any = None) -> None:
        pass

    def on_end(self, span: ReadableSpan) -> None:
        pass

    def shutdown(self) -> None:
        pass

    def force_flush(self, timeout_millis: int = 30000) -> bool:
        return True


class _InlineImages:
    """Keeps images in the telemetry as data URLs instead of uploading them to TraceLoop."""

    async def aupload_base64_image(
        self, trace_id: str, span_id: str, image_name: str, image_file: str
    ) -> str:
        image_format = image_name.rpartition(".")[2] or "png"
        return f"data:image/{image_format};base64,{image_file}"
