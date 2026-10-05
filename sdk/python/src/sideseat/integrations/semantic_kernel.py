"""Semantic Kernel: switches on its GenAI diagnostics and exports the log records they write."""

from __future__ import annotations

import importlib
import logging
from typing import Any

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

# Semantic Kernel writes every prompt and completion as an INFO record of this logger, never as a
# span attribute, so without a handler the conversation is not exported at all.
_MODEL_DIAGNOSTICS_LOGGER = "semantic_kernel.utils.telemetry.model_diagnostics"


class SemanticKernel(Integration):
    name = "semantic-kernel"
    packages = ("semantic-kernel",)

    def __init__(self) -> None:
        self._handler: Any = None
        self._previous_level: int | None = None

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
        if ctx.logger_provider is None or not ctx.capture_content:
            return
        from opentelemetry.sdk._logs import LoggingHandler

        logger = logging.getLogger(_MODEL_DIAGNOSTICS_LOGGER)
        self._handler = LoggingHandler(logger_provider=ctx.logger_provider)
        logger.addHandler(self._handler)
        if logger.getEffectiveLevel() > logging.INFO:
            self._previous_level = logger.level
            logger.setLevel(logging.INFO)

    def shutdown(self) -> None:
        if self._handler is None:
            return
        logger = logging.getLogger(_MODEL_DIAGNOSTICS_LOGGER)
        logger.removeHandler(self._handler)
        if self._previous_level is not None:
            logger.setLevel(self._previous_level)
            self._previous_level = None
        self._handler = None
