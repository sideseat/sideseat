"""Microsoft Agent Framework: switches on its built-in OpenTelemetry instrumentation."""

from __future__ import annotations

from sideseat.integrations._base import Integration, SetupContext


class AgentFramework(Integration):
    name = "agent-framework"
    packages = ("agent-framework-core", "agent-framework")

    def instrument(self, ctx: SetupContext) -> None:
        from agent_framework.observability import OBSERVABILITY_SETTINGS

        OBSERVABILITY_SETTINGS.enable_instrumentation = True
        OBSERVABILITY_SETTINGS.enable_sensitive_data = ctx.capture_content
