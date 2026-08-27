#!/usr/bin/env bash
#
# Run the Claude Code CLI with OpenTelemetry export to SideSeat.
#
# Claude Code does emit spans, contrary to what this script used to say - but only behind two
# beta flags, and message content behind the second one. Configured for metrics and logs alone,
# as it was, it sent SideSeat the two signals SideSeat accepts but does not persist, so nothing
# appeared in the UI. The variables below are the same set the Claude Agent SDK sample suite
# uses (misc/samples/python/claude-agent-sdk/telemetry_setup.py); a test asserts they agree.
#
# Usage:
#   ./run-claude.sh                    # Use defaults
#   PROJECT_ID=myproject ./run-claude.sh
#   SIDESEAT_PORT=5388 ./run-claude.sh
#
# Documentation: https://code.claude.com/docs/en/agent-sdk/observability

set -euo pipefail

# Configuration (override via environment variables)
SIDESEAT_HOST="${SIDESEAT_HOST:-127.0.0.1}"
SIDESEAT_PORT="${SIDESEAT_PORT:-5388}"
PROJECT_ID="${PROJECT_ID:-default}"
AUTH_TOKEN="${AUTH_TOKEN:-}"

# Ingestion base for this project. The OTLP exporter appends /v1/traces itself.
ENDPOINT="http://${SIDESEAT_HOST}:${SIDESEAT_PORT}/otel/${PROJECT_ID}"

# 1. Telemetry at all, then spans: tracing is beta and off without the second flag,
#    which leaves only metrics and logs.
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1

# 2. Second beta tier, and the only way to get conversation text onto spans. Without it the
#    message feed is empty: the model output, user turns, system prompt and tool input
#    attributes are simply not emitted. BETA_TRACING_ENDPOINT takes the base URL - this
#    exporter appends its own suffix, and a full /v1/traces path 404s.
export ENABLE_BETA_TRACING_DETAILED=1
export BETA_TRACING_ENDPOINT="${ENDPOINT}"

# 3. Traces to SideSeat. Never "console": the CLI writes telemetry to stdout, which is the
#    Agent SDK's message channel. Metrics and logs are off because SideSeat accepts but does
#    not persist them - the cost figures on the claude_code.api_request log event are lost as
#    a result, while token counts on llm_request spans survive.
export OTEL_TRACES_EXPORTER=otlp
export OTEL_METRICS_EXPORTER=none
export OTEL_LOGS_EXPORTER=none
export OTEL_EXPORTER_OTLP_TRACES_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_TRACES_ENDPOINT="${ENDPOINT}/v1/traces"
export OTEL_SERVICE_NAME="${OTEL_SERVICE_NAME:-claude-code}"

# 4. Message content on spans. OTEL_LOG_TOOL_CONTENT is left off deliberately: it adds whole
#    tool outputs to spans, which for a file read is the entire file.
export OTEL_LOG_USER_PROMPTS=1
export OTEL_LOG_TOOL_DETAILS=1

# 5. Authentication header (if a token is provided)
if [[ -n "${AUTH_TOKEN}" ]]; then
    export OTEL_EXPORTER_OTLP_TRACES_HEADERS="Authorization=Bearer ${AUTH_TOKEN}"
fi

# 6. Short export interval so a brief session's spans arrive before it exits; flush-on-exit is
#    best-effort.
export OTEL_TRACES_EXPORT_INTERVAL="${OTEL_TRACES_EXPORT_INTERVAL:-1000}"

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
