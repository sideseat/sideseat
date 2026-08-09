"""Telemetry setup for Claude Agent SDK samples.

Unlike every other sample in this tree, there is no instrumentor to install. The
Agent SDK spawns the Claude Code CLI as a child process, and the CLI carries its own
OpenTelemetry instrumentation. Configuration is therefore a dict of environment
variables handed to ClaudeAgentOptions(env=...), not a library patch.

Two halves are wired here:

1. Host process - a SideSeat client so `client.trace(...)` produces a root span. The
   Agent SDK injects TRACEPARENT from the active span into the subprocess, so the
   CLI's `claude_code.interaction` span becomes its child and the whole run lands in
   one trace.
2. Subprocess - the env dict below. Traces are beta, so both
   CLAUDE_CODE_ENABLE_TELEMETRY and CLAUDE_CODE_ENHANCED_TELEMETRY_BETA are required.
"""

import os

from sideseat import Frameworks, SideSeat
from sideseat.config import Config
from sideseat.telemetry.setup import build_endpoint, build_headers

# service.name for host-process spans. Set explicitly because misc/.env sets
# OTEL_SERVICE_NAME=telemetry-sample, which would otherwise win and leave the
# host span unclassified by the server's framework detection.
SERVICE_NAME = "claude-agent-sdk"

# Short intervals so spans reach the collector before a short sample exits. The CLI
# flushes on clean exit, but the flush is bounded by a timeout and lossy if the
# process is killed. Defaults are 5s for traces/logs and 60s for metrics.
EXPORT_INTERVAL_MS = "1000"


def _format_headers(headers: dict[str, str]) -> str:
    """Render headers in the OTEL_EXPORTER_OTLP_HEADERS format (k=v,k=v)."""
    return ",".join(f"{k}={v}" for k, v in headers.items())


def setup_telemetry(use_sideseat: bool = False, model_id: str | None = None):
    """Initialize telemetry.

    Returns (client, env) where env is passed to ClaudeAgentOptions(env=...).
    """
    client = None
    if use_sideseat:
        client = SideSeat(
            framework=Frameworks.ClaudeAgentSDK,
            service_name=SERVICE_NAME,
        )

    env = _build_bedrock_env(model_id)
    env.update(_build_otel_env(client))
    return client, env


def _build_bedrock_env(model_id: str | None) -> dict[str, str]:
    """Route the CLI at Amazon Bedrock using the ambient AWS credential chain."""
    env = {
        "CLAUDE_CODE_USE_BEDROCK": "1",
        "AWS_REGION": os.getenv("AWS_REGION", "us-east-1"),
    }
    if model_id:
        # On Bedrock, background tasks (session titles and similar) fall back to the
        # default Sonnet model, which may not be enabled in the account. Pin the
        # Haiku slot to the sample's model so everything runs on one known-good ID.
        env["ANTHROPIC_MODEL"] = model_id
        env["ANTHROPIC_DEFAULT_HAIKU_MODEL"] = model_id
    return env


def _build_otel_env(client: SideSeat | None) -> dict[str, str]:
    """Build the CLI's OTLP exporter configuration."""
    # Reuse the SDK's own endpoint builder in both branches: it handles an endpoint
    # that already carries a path (e.g. http://host/otel/myproject), which naive
    # concatenation would turn into a doubled path and a silent 404.
    config = client.config if client is not None else Config.create()
    traces_endpoint = build_endpoint(config, "traces")
    headers = build_headers(config)

    # The base URL, not the /v1/traces path: this exporter appends its own suffix
    # and a full path 404s.
    beta_tracing_endpoint = traces_endpoint.removesuffix("/v1/traces")

    env = {
        "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
        # Span tracing is beta and off without this, leaving only metrics and logs.
        "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA": "1",
        # Second beta tier, and the only way to get conversation text onto spans:
        # response.model_output (assistant reply), new_context (user turn),
        # user_system_prompt and tool_input are emitted only when this is on.
        # Without it the SideSeat message feed stays empty.
        "ENABLE_BETA_TRACING_DETAILED": "1",
        "BETA_TRACING_ENDPOINT": beta_tracing_endpoint,
        # Never "console": the CLI writes telemetry to stdout, which is the Agent
        # SDK's message channel, and would corrupt the stream.
        "OTEL_TRACES_EXPORTER": "otlp",
        # SideSeat accepts metrics and logs but only persists traces, so exporting
        # them would burn bandwidth for data that is dropped. The cost figures on
        # the claude_code.api_request log event are lost as a result; token counts
        # on claude_code.llm_request spans survive.
        "OTEL_METRICS_EXPORTER": "none",
        "OTEL_LOGS_EXPORTER": "none",
        "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL": "http/protobuf",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": traces_endpoint,
        "OTEL_TRACES_EXPORT_INTERVAL": EXPORT_INTERVAL_MS,
        "OTEL_SERVICE_NAME": "claude-code",
        # Content is redacted by default, which leaves the SideSeat message feed
        # empty. These are sample runs against local scratch dirs, so opt in.
        "OTEL_LOG_USER_PROMPTS": "1",
        "OTEL_LOG_TOOL_DETAILS": "1",
        # OTEL_LOG_TOOL_CONTENT is deliberately off: it adds a tool.output span event
        # carrying the same result that detailed tracing already reports through
        # new_context, and that copy has no tool_use_id to pair it with its call.
        # Enable it only when running without detailed beta tracing.
        # Surface exporter failures through the stderr callback instead of the CLI
        # silently dropping telemetry.
        "CLAUDE_CODE_OTEL_DIAG_STDERR": "1",
    }
    if headers:
        env["OTEL_EXPORTER_OTLP_TRACES_HEADERS"] = _format_headers(headers)
    return env
