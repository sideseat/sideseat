#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# suite|Python version. Agent Framework currently supports Python 3.12 only;
# the other sample projects support 3.12-3.13 and are exercised on 3.13.
SUITES=(
  "strands|3.13"
  "langgraph|3.13"
  "crewai|3.13"
  "adk|3.13"
  "autogen|3.13"
  "openai-agents|3.13"
  "agent-framework|3.12"
  "claude-agent-sdk|3.13"
  "anthropic|3.13"
  "openai|3.13"
  "google-genai|3.13"
  "pydantic-ai|3.13"
  "bedrock|3.13"
)

for entry in "${SUITES[@]}"; do
  IFS='|' read -r suite python_version <<<"$entry"
  project="examples/python/$suite"
  for mode in native sdk; do
    SIDESEAT_ENDPOINT=http://127.0.0.1:1 \
      uv run --locked --python "$python_version" --project "$project" \
      python scripts/python-framework-smoke.py "$project" "$mode"
  done
done
