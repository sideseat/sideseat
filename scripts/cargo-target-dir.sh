#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"

target_dir="$(
  cargo metadata --locked --no-deps --format-version 1 |
    node -e '
const fs = require("node:fs");
const path = require("node:path");
const metadata = JSON.parse(fs.readFileSync(0, "utf8"));
process.stdout.write(path.resolve(metadata.target_directory) + "\n");
'
)"
if [[ -z "$target_dir" ]]; then
  echo "[cargo-target] refusing empty Cargo target directory" >&2
  exit 1
fi
if [[ ! -e "$target_dir" ]]; then
  printf '%s\n' "$target_dir"
  exit 0
fi
if [[ -L "$target_dir" ]]; then
  echo "[cargo-target] refusing symbolic-link Cargo target directory: $target_dir" >&2
  exit 1
fi
if [[ ! -d "$target_dir" ]]; then
  echo "[cargo-target] refusing Cargo target that is not a directory: $target_dir" >&2
  exit 1
fi

target_dir="$(cd "$target_dir" && pwd -P)"

contains_path() {
  local parent="${1%/}"
  local child="${2%/}"
  [[ "$child" == "$parent" || "$child" == "$parent/"* ]]
}

if contains_path "$target_dir" "$repo_root"; then
  echo "[cargo-target] refusing Cargo target that contains the repository: $target_dir" >&2
  exit 1
fi

home_dir="${HOME:-}"
if [[ -n "$home_dir" && -d "$home_dir" ]]; then
  home_dir="$(cd "$home_dir" && pwd -P)"
  if contains_path "$target_dir" "$home_dir"; then
    echo "[cargo-target] refusing Cargo target that contains the home directory: $target_dir" >&2
    exit 1
  fi
fi

printf '%s\n' "$target_dir"
