#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
uv_command="${UV_COMMAND:-uv}"
project_dir="$repo_root/examples/python/sdk-conformance"
run_dir="$(mktemp -d "${TMPDIR:-/tmp}/sideseat-python-conformance.XXXXXX")"
recorder_pid=""
python_version=""
IFS= read -r python_version <"$project_dir/.python-version"
python_version="${PYTHON_VERSION:-$python_version}"

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
    echo "[python-conformance] recorder failed to start for $label" >&2
    return 1
  fi
  local port
  port="$(sed -n 's#.*listening on http://127.0.0.1:\([0-9]*\).*#\1#p' "$recorder_log")"

  SIDESEAT_ENDPOINT="http://127.0.0.1:$port" \
  SIDESEAT_PROJECT_ID=default \
    "$uv_command" run \
      --locked \
      --project "$project_dir" \
      --python "$python_version" \
      python "$project_dir/conformance.py" "$mode"

  cleanup_recorder

  local fixture_dir="$repo_root/server/tests/fixtures/messages/$label"
  local captured
  captured="$(find "$fixture_dir" -maxdepth 1 -type f -name 'req-*' | wc -l | tr -d ' ')"
  if [[ "$captured" -lt 1 ]]; then
    cat "$recorder_log"
    echo "[python-conformance] $label exported no OTLP trace requests" >&2
    return 1
  fi
  echo "[python-conformance] $label: $captured request(s)"
}

command -v "$uv_command" >/dev/null 2>&1 || {
  echo "[python-conformance] uv was not found: $uv_command" >&2
  exit 1
}

"$uv_command" sync \
  --locked \
  --project "$project_dir" \
  --python "$python_version"

capture_mode python/native/canonical otel
capture_mode python/sdk/canonical sdk

echo "[python-conformance] review and record expectations:"
echo "  scripts/fixtures/review-goldens.py python/native/canonical"
echo "  scripts/fixtures/review-goldens.py python/sdk/canonical"
echo "  UPDATE_GOLDENS=1 cargo test --locked -p sideseat-server --test message_goldens message_goldens"
