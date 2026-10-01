"""Telemetry setup for TraceLoop captures."""

from typing import Any

from opentelemetry.sdk.trace import SpanProcessor
from sideseat import Frameworks
from traceloop.sdk import Traceloop
from traceloop.sdk.instruments import Instruments

from common.telemetry import NativeTraceClient, setup_base_telemetry

SERVICE_NAME = "traceloop-sample"


class _EnrichmentOnlyProcessor(SpanProcessor):
    """Allow TraceLoop enrichment while the native provider owns export."""

    def on_start(self, span: Any, parent_context: Any = None) -> None:
        pass

    def on_end(self, span: Any) -> None:
        pass

    def shutdown(self) -> None:
        pass

    def force_flush(self, timeout_millis: int = 30000) -> bool:
        return True


class _InlineImageUploader:
    """Preserve data URLs rather than uploading fixture content externally."""

    async def aupload_base64_image(
        self,
        trace_id: str,
        span_id: str,
        image_name: str,
        image_file: str,
    ) -> str:
        del trace_id, span_id
        image_format = image_name.rpartition(".")[2] or "png"
        return f"data:image/{image_format};base64,{image_file}"


def _instrument_native(provider: Any = None) -> None:
    """Initialize TraceLoop on the already configured native provider."""
    del provider
    Traceloop.init(
        app_name=SERVICE_NAME,
        processor=_EnrichmentOnlyProcessor(),
        instruments={Instruments.OPENAI},
        block_instruments={Instruments.REQUESTS, Instruments.URLLIB3},
        image_uploader=_InlineImageUploader(),
        use_attributes=True,
    )


def setup_telemetry(use_sideseat: bool = False) -> Any:
    """Configure TraceLoop directly or through SideSeat."""
    owner = setup_base_telemetry(
        instrumentor=_instrument_native,
        use_sideseat=use_sideseat,
        framework=Frameworks.TraceLoop,
    )
    if use_sideseat:
        return owner
    return NativeTraceClient(owner, SERVICE_NAME)
