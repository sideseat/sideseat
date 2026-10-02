"""Haystack: connects its native tracing API to SideSeat's provider."""

from __future__ import annotations

import importlib

from sideseat.integrations._base import Integration, SetupContext


class Haystack(Integration):
    name = "haystack"
    packages = ("haystack-ai",)
    extra = "haystack"

    def instrument(self, ctx: SetupContext) -> None:
        tracing = importlib.import_module("haystack.tracing")
        integration = importlib.import_module("haystack_integrations.tracing.opentelemetry")
        assert ctx.tracer_provider is not None
        # Haystack reads its content opt-in when the tracing module is imported, which is usually
        # before init, so the environment variable alone would not take effect.
        tracing.tracer.is_content_tracing_enabled = ctx.capture_content
        tracing.enable_tracing(
            integration.OpenTelemetryTracer(ctx.tracer_provider.get_tracer("haystack"))
        )

    def shutdown(self) -> None:
        importlib.import_module("haystack.tracing").disable_tracing()
