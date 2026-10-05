#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
child_pids=()

terminate_children() {
  local pid
  local attempt
  local alive

  for pid in "${child_pids[@]}"; do
    if kill -0 "$pid" 2>/dev/null; then
      kill -TERM "$pid" 2>/dev/null || true
    fi
  done

  for ((attempt = 0; attempt < 50; attempt++)); do
    alive=false
    for pid in "${child_pids[@]}"; do
      if kill -0 "$pid" 2>/dev/null; then
        alive=true
        break
      fi
    done
    if [[ "$alive" == false ]]; then
      break
    fi
    sleep 0.1
  done

  for pid in "${child_pids[@]}"; do
    if kill -0 "$pid" 2>/dev/null; then
      kill -KILL "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
  done
}

cleanup() {
  local exit_code=$?
  trap - EXIT INT TERM HUP
  terminate_children
  exit "$exit_code"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

cd "$repo_root"
echo "[dev] Starting server (port 5388) and web (port 5389)..."

./scripts/dev-server.sh "$@" &
server_pid=$!
child_pids+=("$server_pid")

npm --prefix web run dev &
web_pid=$!
child_pids+=("$web_pid")

while true; do
  if ! kill -0 "$server_pid" 2>/dev/null; then
    set +e
    wait "$server_pid"
    child_status=$?
    set -e
    echo "Error: development server exited with status $child_status" >&2
    if ((child_status == 0)); then
      child_status=1
    fi
    exit "$child_status"
  fi
  if ! kill -0 "$web_pid" 2>/dev/null; then
    set +e
    wait "$web_pid"
    child_status=$?
    set -e
    echo "Error: web development server exited with status $child_status" >&2
    if ((child_status == 0)); then
      child_status=1
    fi
    exit "$child_status"
  fi
  sleep 0.2
done
