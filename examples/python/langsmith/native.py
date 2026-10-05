"""LangSmith's documented OpenTelemetry mode, exporting to an OTLP endpoint instead of LangSmith.

With ``LANGSMITH_TRACING_MODE=otel`` LangSmith converts each run LangChain and LangGraph report into an
OpenTelemetry span on the global tracer provider and sends nothing to its own API. The application's
provider exports over OTLP, so it reaches any OpenTelemetry backend.
"""

import os

from harness.telemetry import NativeTelemetry


def configure(native: NativeTelemetry) -> None:
    provider = native.provider()
    os.environ["LANGSMITH_TRACING"] = "true"
    os.environ["LANGSMITH_TRACING_MODE"] = "otel"

    def shutdown() -> None:
        # LangSmith converts runs to spans on a background thread, so it is drained before the
        # provider flushes; LangSmith's documentation says to call this before a process exits.
        from langchain_core.tracers.langchain import wait_for_all_tracers

        wait_for_all_tracers()
        provider.force_flush(30_000)
        provider.shutdown()

    native.hand_over(trace=native._otel_trace, shutdown=shutdown)
