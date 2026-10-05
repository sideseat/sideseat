"""Microsoft Agent Framework: switches on its built-in OpenTelemetry instrumentation."""

from __future__ import annotations

from typing import Any

from sideseat.integrations._base import Integration, SetupContext


class AgentFramework(Integration):
    name = "agent-framework"
    packages = ("agent-framework-core", "agent-framework")

    def __init__(self) -> None:
        self._previous: tuple[Any, Any] | None = None

    def instrument(self, ctx: SetupContext) -> None:
        from agent_framework.observability import OBSERVABILITY_SETTINGS

        self._previous = (
            OBSERVABILITY_SETTINGS.enable_instrumentation,
            OBSERVABILITY_SETTINGS.enable_sensitive_data,
        )
        OBSERVABILITY_SETTINGS.enable_instrumentation = True
        OBSERVABILITY_SETTINGS.enable_sensitive_data = ctx.capture_content

    def shutdown(self) -> None:
        if self._previous is None:
            return
        from agent_framework.observability import OBSERVABILITY_SETTINGS

        (
            OBSERVABILITY_SETTINGS.enable_instrumentation,
            OBSERVABILITY_SETTINGS.enable_sensitive_data,
        ) = self._previous
        self._previous = None
