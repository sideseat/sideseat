#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
npm_command="${NPM_COMMAND:-npm}"
sdk_dir="$repo_root/sdk/js"
project_dir="$repo_root/examples/javascript/sdk-conformance"
run_dir="$(mktemp -d "${TMPDIR:-/tmp}/sideseat-javascript-conformance.XXXXXX")"
recorder_pid=""
node_version=""
IFS= read -r node_version <"$project_dir/.node-version"
node_bin="$project_dir/node_modules/.bin/node"
tsx_cli="$project_dir/node_modules/tsx/dist/cli.mjs"

cleanup_recorder() {
  if [[ -n "$recorder_pid" ]] && kill -0 "$recorder_pid" 2>/dev/null; then
    kill -TERM "$recorder_pid" 2>/dev/null || true
    wait "$recorder_pid" 2>/dev/null || true
  fi
  recorder_pid=""
}

cleanup() {
  cleanup_recorder
  find "$run_dir" -depth -delete
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

capture_mode() {
  local label="$1"
  local mode="$2"
  local recorder_log="$run_dir/recorder-$mode.log"

  python3 "$repo_root/scripts/fixtures/record-otlp.py" \
    --no-forward \
    --label "$label" \
    --port 0 >"$recorder_log" 2>&1 &
  recorder_pid=$!

  local ready=0
  for _ in $(seq 1 100); do
    if grep -q "listening on" "$recorder_log"; then
      ready=1
      break
    fi
    kill -0 "$recorder_pid" 2>/dev/null || break
    sleep 0.1
  done
  if [[ "$ready" != "1" ]]; then
    cat "$recorder_log"
    echo "[javascript-conformance] recorder failed to start for $label" >&2
    return 1
  fi
  local port
  port="$(sed -n 's#.*listening on http://127.0.0.1:\([0-9]*\).*#\1#p' "$recorder_log")"

  SIDESEAT_ENDPOINT="http://127.0.0.1:$port" \
  SIDESEAT_PROJECT_ID=default \
    "$node_bin" "$tsx_cli" "$project_dir/conformance.ts" "$mode"

  cleanup_recorder

  local fixture_dir="$repo_root/server/tests/fixtures/messages/$label"
  local captured
  captured="$(find "$fixture_dir" -maxdepth 1 -type f -name 'req-*' | wc -l | tr -d ' ')"
  if [[ "$captured" -lt 1 ]]; then
    cat "$recorder_log"
    echo "[javascript-conformance] $label exported no OTLP trace requests" >&2
    return 1
  fi
  echo "[javascript-conformance] $label: $captured request(s)"
}

command -v "$npm_command" >/dev/null 2>&1 || {
  echo "[javascript-conformance] npm was not found: $npm_command" >&2
  exit 1
}

"$npm_command" ci --prefix "$sdk_dir"
"$npm_command" run build --prefix "$sdk_dir"
"$npm_command" ci --prefix "$project_dir"

if [[ "$("$node_bin" --version)" != "v$node_version" ]]; then
  echo "[javascript-conformance] expected Node v$node_version from lockfile" >&2
  exit 1
fi

"$npm_command" run typecheck --prefix "$project_dir"
"$npm_command" run format:check --prefix "$project_dir"

capture_mode javascript/native/canonical otel
capture_mode javascript/sdk/canonical sdk

echo "[javascript-conformance] review and record expectations:"
echo "  scripts/fixtures/review-goldens.py javascript/native/canonical"
echo "  scripts/fixtures/review-goldens.py javascript/sdk/canonical"
echo "  UPDATE_GOLDENS=1 cargo test --locked -p sideseat-server --test message_goldens message_goldens"
