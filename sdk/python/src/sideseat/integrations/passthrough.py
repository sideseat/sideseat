"""Producers that emit OpenTelemetry through the global provider and need nothing switched on."""

from __future__ import annotations

from sideseat.integrations._base import Integration


class Langflow(Integration):
    name = "langflow"
    packages = ("langflow",)


class OpenInference(Integration):
    """Applications that create OpenInference spans directly, without a framework instrumentor."""

    name = "openinference"
    packages = ("openinference-instrumentation",)
    detectable = False
