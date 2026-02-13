#!/usr/bin/env bash
#
# Run Claude Code with OpenTelemetry export to SideSeat
#
# NOTE: Claude Code exports metrics and logs (usage data), NOT traces.
#       For traces, use an instrumented AI framework (Strands, LangChain, etc.)
#
# Usage:
#   ./run-claude.sh                    # Use defaults
#   PROJECT_ID=myproject ./run-claude.sh
#   SIDESEAT_PORT=5388 ./run-claude.sh
#
# Documentation: https://docs.anthropic.com/en/docs/claude-code/telemetry

set -euo pipefail

# Configuration (override via environment variables)
SIDESEAT_HOST="${SIDESEAT_HOST:-127.0.0.1}"
SIDESEAT_PORT="${SIDESEAT_PORT:-5388}"
PROJECT_ID="${PROJECT_ID:-default}"
AUTH_TOKEN="${AUTH_TOKEN:-}"

# Build endpoint URL
ENDPOINT="http://${SIDESEAT_HOST}:${SIDESEAT_PORT}/otel/${PROJECT_ID}"

# 1. Enable Claude Code telemetry
export CLAUDE_CODE_ENABLE_TELEMETRY=1

# 2. Configure exporters (Claude Code currently uses metrics + logs only)
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp

# 3. Configure OTLP endpoint
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_ENDPOINT="${ENDPOINT}"

# 4. Set authentication header (if token provided)
if [[ -n "${AUTH_TOKEN}" ]]; then
    export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${AUTH_TOKEN}"
fi

# 5. Reduce export intervals for faster feedback during development
export OTEL_METRIC_EXPORT_INTERVAL="${OTEL_METRIC_EXPORT_INTERVAL:-1000}"
export OTEL_LOGS_EXPORT_INTERVAL="${OTEL_LOGS_EXPORT_INTERVAL:-1000}"

# Verify claude command exists
if ! command -v claude &> /dev/null; then
    echo "Error: 'claude' command not found. Install Claude Code first." >&2
    exit 1
fi

# Display configuration
echo "SideSeat endpoint: ${ENDPOINT}"
echo "Starting Claude Code..."
echo ""

# Run Claude Code (pass through any arguments)
exec claude "$@"
