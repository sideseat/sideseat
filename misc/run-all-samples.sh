#!/usr/bin/env bash
# Runs every sample that can reach a provider with the credentials present, then verifies
# via the query API that each run actually produced correct data - not just exit code 0.
#
# Suites needing OPENAI_API_KEY / ANTHROPIC_API_KEY are omitted; with AWS credentials only
# they cannot reach a model at all. See misc/.env.example.
#
# Do not rebuild the server while this runs: the dev server restarts on source changes and
# spans exported during a restart are dropped, which looks like a sample failure.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
API="http://127.0.0.1:5388/api/v1/project/default/otel"
COOKIES="${SIDESEAT_COOKIES:-/tmp/ss-cookies.txt}"
OUT="${1:-/tmp/sample-results.tsv}"
: > "$OUT"

# Newest trace start_time currently stored, used as a per-sample freshness watermark.
watermark() {
  curl -s -b "$COOKIES" "$API/traces?limit=1" |
    python3 -c 'import sys,json; d=json.load(sys.stdin).get("data") or [{}]; print(d[0].get("start_time","1970-01-01T00:00:00"))' \
      2>/dev/null || echo "1970-01-01T00:00:00"
}

# verify <label> <expect_error:yes|no> <watermark_iso>
# Judges only a trace that started after the watermark. Taking "newest overall" instead
# would silently re-validate the previous run's trace whenever a run exports nothing,
# turning real failures into passes.
verify() {
  sleep 8
  SS_LABEL="$1" SS_EXPECT="$2" SS_API="$API" SS_COOKIES="$COOKIES" SS_OUT="$OUT" SS_MARK="$3" \
    python3 "$ROOT/misc/verify-sample-trace.py"
}

run_py() {
  # run_py <suite> <script> <sample> [extra args...]
  local suite="$1" script="$2" sample="$3"; shift 3
  local expect_err=no; [[ "$sample" == error* ]] && expect_err=yes
  echo "### py/$suite/$sample"
  local mark; mark="$(watermark)"
  # An `error` sample is meant to fail, so its exit code carries no information.
  if timeout 900 uv run --directory "$ROOT/misc/samples/python/$suite" "$script" "$sample" --sideseat "$@" \
    >"/tmp/s-$suite-$sample.log" 2>&1 || [[ "$expect_err" == yes ]]; then
    verify "py/$suite/$sample" "$expect_err" "$mark"
  else
    printf '%s\tRUN_FAIL\t0\t0\t0\t0\t\t%s\n' "py/$suite/$sample" \
      "$(tail -3 "/tmp/s-$suite-$sample.log" | tr '\n' ' ' | cut -c1-160)" >> "$OUT"
  fi
}

run_js() {
  local suite="$1" sample="$2"
  local expect_err=no; [[ "$sample" == error* ]] && expect_err=yes
  echo "### js/$suite/$sample"
  local mark; mark="$(watermark)"
  if (cd "$ROOT/misc/samples/js" && timeout 900 npm run "$suite" -- "$sample" --sideseat) \
    >"/tmp/s-js-$suite-$sample.log" 2>&1 || [[ "$expect_err" == yes ]]; then
    verify "js/$suite/$sample" "$expect_err" "$mark"
  else
    printf '%s\tRUN_FAIL\t0\t0\t0\t0\t\t%s\n' "js/$suite/$sample" \
      "$(tail -3 "/tmp/s-js-$suite-$sample.log" | tr '\n' ' ' | cut -c1-160)" >> "$OUT"
  fi
}

# --- Bedrock-backed Python suites ---
for s in agent_core error files image_gen mcp_tools rag_local reasoning structured_output swarm tool_use; do
  run_py strands strands "$s"
done
for s in custom_tools error mcp_tools multi_turn permissions reasoning structured_output subagents tool_use; do
  run_py claude-agent-sdk claude-agent-sdk "$s"
done
for s in converse document error invoke_model multi_turn session; do
  run_py bedrock bedrock "$s"
done
for s in agent_core error files image_gen mcp_tools rag_local reasoning structured_output swarm tool_use; do
  run_py adk telemetry-adk "$s"
done
# The suite installs a `crewai` script but so does the crewai package, and the real CLI
# wins; telemetry-crewai is the collision-free alias.
for s in agent_core error files image_gen mcp_tools rag_local reasoning structured_output swarm tool_use; do
  run_py crewai telemetry-crewai "$s"
done
for s in error files image_gen mcp_tools rag_local reasoning structured_output swarm tool_use; do
  run_py langgraph langgraph "$s"
done

# --- Bedrock-backed JS suites ---
for s in tool-use mcp-tools structured-output reasoning custom-tools subagents multi-turn permissions error; do
  run_js claude-agent-sdk "$s"
done
for s in tool-use mcp-tools structured-output reasoning rag-local files image-gen swarm error; do
  run_js strands "$s"
done
for s in tool-use multi-step structured-output reasoning rag-local files image-gen error; do
  run_js vercel-ai "$s"
done

echo "DONE -> $OUT"
