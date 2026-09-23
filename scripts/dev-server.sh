#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
args=("$@")
has_config=false

absolute_config_path() {
  local path="$1"

  case "$path" in
    \~) printf '%s\n' "$HOME" ;;
    \~/*) printf '%s/%s\n' "$HOME" "${path#\~/}" ;;
    /*) printf '%s\n' "$path" ;;
    *) printf '%s/%s\n' "$repo_root" "$path" ;;
  esac
}

for ((index = 0; index < ${#args[@]}; index++)); do
  case "${args[index]}" in
    --config)
      has_config=true
      ((index + 1 < ${#args[@]})) || {
        echo "Error: --config requires a path" >&2
        exit 2
      }
      config_path="${args[index + 1]}"
      args[index + 1]="$(absolute_config_path "$config_path")"
      index=$((index + 1))
      ;;
    --config=*)
      has_config=true
      config_path="${args[index]#--config=}"
      if [[ -z "$config_path" ]]; then
        echo "Error: --config requires a path" >&2
        exit 2
      fi
      args[index]="--config=$(absolute_config_path "$config_path")"
      ;;
  esac
done

server_env=(
  "SIDESEAT_LOG=debug"
  "SIDESEAT_DATA_DIR=$repo_root/.sideseat"
)
if [[ -n "${SIDESEAT_SECRETS_BACKEND:-}" ]]; then
  server_env+=("SIDESEAT_SECRETS_BACKEND=$SIDESEAT_SECRETS_BACKEND")
elif [[ "$has_config" == false ]]; then
  server_env+=("SIDESEAT_SECRETS_BACKEND=file")
fi

cd "$repo_root/server"
if command -v watchexec >/dev/null 2>&1; then
  exec env "${server_env[@]}" watchexec -r -e rs,toml -- \
    cargo run --locked -- "${args[@]}"
fi

if command -v cargo-watch >/dev/null 2>&1; then
  printf -v watch_command '%q ' run --locked -- "${args[@]}"
  exec env "${server_env[@]}" cargo watch -x "${watch_command% }"
fi

echo "No watch tool found; starting the server without reload." >&2
exec env "${server_env[@]}" cargo run --locked -- "${args[@]}"
