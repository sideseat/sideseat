#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
dotnet_command="${DOTNET_COMMAND:-dotnet}"
project="$repo_root/examples/dotnet/conformance/SideSeat.Conformance.csproj"
run_dir="$(mktemp -d "${TMPDIR:-/tmp}/sideseat-dotnet-conformance.XXXXXX")"
recorder_pid=""

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
    echo "[dotnet-conformance] recorder failed to start for $label" >&2
    return 1
  fi
  local port
  port="$(sed -n 's#.*listening on http://127.0.0.1:\([0-9]*\).*#\1#p' "$recorder_log")"

  SIDESEAT_ENDPOINT="http://127.0.0.1:$port" \
  SIDESEAT_PROJECT_ID=default \
    "$dotnet_command" run \
      --project "$project" \
      --configuration Release \
      --no-build \
      -- "$mode"

  # Batch processors flush before the process exits, but let the HTTP handler finish
  # writing the request before terminating the recorder.
  sleep 1
  cleanup_recorder

  local fixture_dir="$repo_root/server/tests/fixtures/messages/$label"
  local captured
  captured="$(find "$fixture_dir" -maxdepth 1 -type f -name 'req-*' | wc -l | tr -d ' ')"
  if [[ "$captured" -lt 1 ]]; then
    cat "$recorder_log"
    echo "[dotnet-conformance] $label exported no OTLP trace requests" >&2
    return 1
  fi
  echo "[dotnet-conformance] $label: $captured request(s)"
}

command -v "$dotnet_command" >/dev/null 2>&1 || {
  echo "[dotnet-conformance] dotnet was not found: $dotnet_command" >&2
  exit 1
}

"$dotnet_command" restore "$project" --locked-mode
"$dotnet_command" build "$project" --configuration Release --no-restore -warnaserror

capture_mode dotnet/native/canonical otel
capture_mode dotnet/sdk/canonical sdk

echo "[dotnet-conformance] review and record expectations:"
echo "  scripts/fixtures/review-goldens.py dotnet/native/canonical"
echo "  scripts/fixtures/review-goldens.py dotnet/sdk/canonical"
echo "  UPDATE_GOLDENS=1 cargo test --locked -p sideseat-server --test message_goldens message_goldens"
