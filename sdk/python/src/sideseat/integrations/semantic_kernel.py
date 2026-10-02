"""Semantic Kernel: switches on its GenAI diagnostics."""

from __future__ import annotations

import importlib

from sideseat.integrations._base import Integration, SetupContext
from sideseat.integrations._util import default_env

# Each diagnostics module snapshots its settings at import time. Applications usually import
# Semantic Kernel before calling init, so the environment alone cannot enable an already-loaded
# module.
_DIAGNOSTICS_MODULES = (
    "semantic_kernel.utils.telemetry.agent_diagnostics.decorators",
    "semantic_kernel.utils.telemetry.model_diagnostics.decorators",
    "semantic_kernel.utils.telemetry.model_diagnostics.function_tracer",
)


class SemanticKernel(Integration):
    name = "semantic-kernel"
    packages = ("semantic-kernel",)

    def prepare(self, ctx: SetupContext) -> None:
        default_env("SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS", "true")
        default_env(
            "SEMANTICKERNEL_EXPERIMENTAL_GENAI_ENABLE_OTEL_DIAGNOSTICS_SENSITIVE",
            "true" if ctx.capture_content else "false",
        )

    def instrument(self, ctx: SetupContext) -> None:
        for module_name in _DIAGNOSTICS_MODULES:
            settings = importlib.import_module(module_name).MODEL_DIAGNOSTICS_SETTINGS
            settings.enable_otel_diagnostics = True
            settings.enable_otel_diagnostics_sensitive = ctx.capture_content
