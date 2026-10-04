"""The Claude Code CLI's documented telemetry setup: OTEL_* and CLAUDE_CODE_* variables.

The Agent SDK spawns the Claude Code CLI, which carries its own OpenTelemetry instrumentation and
reads its configuration from the environment it inherits. The host process exports its own spans
through a plain provider, and the Agent SDK hands the active span to the CLI as ``TRACEPARENT``.
"""

import os

from harness.telemetry import NativeTelemetry, auth_headers, traces_endpoint


def configure(native: NativeTelemetry) -> None:
    native.provider()
    endpoint = traces_endpoint()
    os.environ.update(
        {
            "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
            # Spans are a beta tier; without it the CLI exports only metrics and logs.
            "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA": "1",
            # The detailed tier is the one that records the conversation on spans.
            "ENABLE_BETA_TRACING_DETAILED": "1",
            "BETA_TRACING_ENDPOINT": endpoint.removesuffix("/v1/traces"),
            # Never "console": the CLI's stdout is the Agent SDK's message channel.
            "OTEL_TRACES_EXPORTER": "otlp",
            "OTEL_METRICS_EXPORTER": "none",
            "OTEL_LOGS_EXPORTER": "none",
            "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL": "http/protobuf",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": endpoint,
            "OTEL_TRACES_EXPORT_INTERVAL": "1000",
            "OTEL_SERVICE_NAME": "claude-code",
            "OTEL_LOG_USER_PROMPTS": "1",
            "OTEL_LOG_TOOL_DETAILS": "1",
        }
    )
    if headers := auth_headers():
        os.environ["OTEL_EXPORTER_OTLP_TRACES_HEADERS"] = ",".join(
            f"{key}={value}" for key, value in headers.items()
        )
