#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

target_dir="$(
  cargo metadata --locked --no-deps --format-version 1 |
    node -e '
const fs = require("node:fs");
const metadata = JSON.parse(fs.readFileSync(0, "utf8"));
process.stdout.write(metadata.target_directory + "\n");
'
)"
if [[ -z "$target_dir" || "$target_dir" == "/" ]]; then
  echo "[clean-stale] refusing unsafe Cargo target directory: $target_dir" >&2
  exit 1
fi

before="$(du -sk "$target_dir" 2>/dev/null | awk '{print $1}')"
before="${before:-0}"

if command -v cargo-sweep >/dev/null 2>&1; then
  echo "[clean-stale] Removing artifacts from inactive toolchains..."
  cargo sweep --installed
  echo "[clean-stale] Removing artifacts unused for three days..."
  cargo sweep --time 3
else
  echo "[clean-stale] cargo-sweep not installed; keeping non-incremental artifacts."
  echo "[clean-stale] Install it with: cargo install cargo-sweep"
fi

incremental_dirs=0
while IFS= read -r -d '' directory; do
  rm -rf -- "$directory"
  incremental_dirs=$((incremental_dirs + 1))
done < <(find "$target_dir" -type d -name incremental -prune -print0)

after="$(du -sk "$target_dir" 2>/dev/null | awk '{print $1}')"
after="${after:-0}"
echo "[clean-stale] removed $incremental_dirs incremental directories"
echo "[clean-stale] target: $((before / 1024)) MB -> $((after / 1024)) MB"
