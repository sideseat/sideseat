"""Claude Agent SDK: configures the telemetry of the Claude Code CLI it spawns.

The Agent SDK runs the Claude Code CLI as a child process, and the CLI carries its own OpenTelemetry
instrumentation. There is nothing to patch in-process; the CLI is configured through environment
variables. :meth:`ClaudeAgentSDK.instrument` merges them into every ``ClaudeAgentOptions`` the
application creates, without overriding a variable the application set itself. The Agent SDK passes
the active span to the CLI as ``TRACEPARENT``, so the CLI's spans join the caller's trace.
"""

from __future__ import annotations

import functools
from typing import Any

from sideseat._config import Settings
from sideseat.integrations._base import Integration, SetupContext


def cli_environment(settings: Settings) -> dict[str, str]:
    """Environment variables that send the Claude Code CLI's telemetry to SideSeat."""
    env = {
        "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
        # Span tracing is a beta tier; without it the CLI emits only metrics and logs.
        "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA": "1",
        # The second beta tier is the only one that puts the conversation on spans.
        "ENABLE_BETA_TRACING_DETAILED": "1",
        # This exporter appends its own /v1/traces suffix.
        "BETA_TRACING_ENDPOINT": settings.otlp_base,
        # Never "console": the CLI's stdout is the Agent SDK's message channel.
        "OTEL_TRACES_EXPORTER": "otlp",
        "OTEL_METRICS_EXPORTER": "none",
        "OTEL_LOGS_EXPORTER": "none",
        "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL": "http/protobuf",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": settings.signal_endpoint("traces"),
        # Short-lived CLI runs exit before the default five-second batch interval elapses.
        "OTEL_TRACES_EXPORT_INTERVAL": "1000",
        "OTEL_SERVICE_NAME": "claude-code",
        "CLAUDE_CODE_OTEL_DIAG_STDERR": "1",
    }
    if settings.capture_content:
        env["OTEL_LOG_USER_PROMPTS"] = "1"
        env["OTEL_LOG_TOOL_DETAILS"] = "1"
    headers = settings.headers()
    if headers:
        env["OTEL_EXPORTER_OTLP_TRACES_HEADERS"] = ",".join(f"{k}={v}" for k, v in headers.items())
    return env


class ClaudeAgentSDK(Integration):
    name = "claude-agent-sdk"
    packages = ("claude-agent-sdk",)

    def __init__(self) -> None:
        self._original_init: Any = None

    def instrument(self, ctx: SetupContext) -> None:
        from claude_agent_sdk import ClaudeAgentOptions

        telemetry_env = cli_environment(ctx.settings)
        original = ClaudeAgentOptions.__init__

        @functools.wraps(original)
        def init_with_telemetry(options: Any, *args: Any, **kwargs: Any) -> None:
            original(options, *args, **kwargs)
            options.env = {**telemetry_env, **(options.env or {})}

        ClaudeAgentOptions.__init__ = init_with_telemetry
        self._original_init = original

    def shutdown(self) -> None:
        if self._original_init is not None:
            from claude_agent_sdk import ClaudeAgentOptions

            ClaudeAgentOptions.__init__ = self._original_init
            self._original_init = None
