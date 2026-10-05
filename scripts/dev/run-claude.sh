#!/usr/bin/env bash
#
# Run the Claude Code CLI with OpenTelemetry export to SideSeat.
#
# Claude Code emits spans only when both telemetry beta tiers are enabled; detailed message
# content requires the second tier. The variables below match the Claude Agent SDK sample suite
# in examples/python/claude-agent-sdk/native.py.
#
# Usage:
#   ./run-claude.sh                    # Use defaults
#   PROJECT_ID=myproject ./run-claude.sh
#   SIDESEAT_PORT=5388 ./run-claude.sh
#
# Documentation: https://code.claude.com/docs/en/monitoring-usage

set -euo pipefail

# Configuration (override via environment variables)
SIDESEAT_HOST="${SIDESEAT_HOST:-127.0.0.1}"
SIDESEAT_PORT="${SIDESEAT_PORT:-5388}"
PROJECT_ID="${PROJECT_ID:-default}"
AUTH_TOKEN="${AUTH_TOKEN:-}"

# Ingestion base for this project. The OTLP exporter appends /v1/traces itself.
ENDPOINT="http://${SIDESEAT_HOST}:${SIDESEAT_PORT}/otel/${PROJECT_ID}"

# 1. Telemetry at all, then spans: tracing is beta and off without the second flag, which leaves
#    metrics and log events with no exported span to join them to a turn.
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1

# 2. Second beta tier: the conversation on spans as well, and the full system prompt. Without it the
#    log events still carry the prompt, the replies and the tool calls; interactive sessions get this
#    tier only when the organisation is allowlisted. BETA_TRACING_ENDPOINT takes the base URL - this
#    exporter appends its own suffix, and a full /v1/traces path 404s.
export ENABLE_BETA_TRACING_DETAILED=1
export BETA_TRACING_ENDPOINT="${ENDPOINT}"

# 3. All three signals to SideSeat. Never "console": the CLI writes telemetry to stdout, which is the
#    Agent SDK's message channel. Log events carry the conversation where the detailed tier does not,
#    and every one names its turn's span; metrics carry cost and usage.
export OTEL_TRACES_EXPORTER=otlp
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_ENDPOINT="${ENDPOINT}"
export OTEL_SERVICE_NAME="${OTEL_SERVICE_NAME:-claude-code}"

# 4. Message content. OTEL_LOG_TOOL_CONTENT adds each tool's output as a span event - for a file read,
#    the whole file - and is the only place a tool's output is exported.
export OTEL_LOG_USER_PROMPTS=1
export OTEL_LOG_TOOL_DETAILS=1
export OTEL_LOG_TOOL_CONTENT="${OTEL_LOG_TOOL_CONTENT:-1}"

# 5. Authentication header (if a token is provided)
if [[ -n "${AUTH_TOKEN}" ]]; then
    export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${AUTH_TOKEN}"
fi

# 6. Short export intervals so a brief session's telemetry arrives before it exits; flush-on-exit is
#    best-effort.
export OTEL_TRACES_EXPORT_INTERVAL="${OTEL_TRACES_EXPORT_INTERVAL:-1000}"
export OTEL_LOGS_EXPORT_INTERVAL="${OTEL_LOGS_EXPORT_INTERVAL:-1000}"
export OTEL_METRIC_EXPORT_INTERVAL="${OTEL_METRIC_EXPORT_INTERVAL:-10000}"

# Verify claude command exists
if ! command -v claude &> /dev/null; then
    echo "Error: 'claude' command not found. Install Claude Code first." >&2
    exit 1
fi

echo "SideSeat endpoint: ${ENDPOINT}"
echo "Starting Claude Code..."
echo ""

# Run Claude Code (pass through any arguments)
exec claude "$@"
