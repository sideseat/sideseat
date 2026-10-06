#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

target_dir="$(bash scripts/dev/cargo-target-dir.sh)"
if [[ ! -d "$target_dir" ]]; then
  echo "[clean-stale] target directory does not exist; nothing to remove"
  exit 0
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

# State nothing has touched for STALE_HOURS is not in use by any running build - a build rewrites the session
# it works on - so it goes even while other builds run: incremental sessions of crate variants no command has
# built lately (each feature set and target kind gets its own), and the per-codegen-unit objects a profile with
# debug info leaves beside its binaries on macOS.
stale_minutes=$(( ${STALE_HOURS:-6} * 60 ))
stale_sessions=0
while IFS= read -r -d '' session; do
  if [ -z "$(find "$session" -type f -mmin "-$stale_minutes" -print -quit 2>/dev/null)" ]; then
    rm -rf -- "$session"
    stale_sessions=$((stale_sessions + 1))
  fi
done < <(find "$target_dir" -path '*/incremental/*' -mindepth 1 -maxdepth 4 -type d -prune -path '*/incremental/*' -print0 2>/dev/null)
stale_objects="$(find "$target_dir" -name '*.rcgu.o' -mmin "+$stale_minutes" -print -delete 2>/dev/null | wc -l | tr -d ' ')"
echo "[clean-stale] removed $stale_sessions incremental sessions and $stale_objects objects untouched for ${STALE_HOURS:-6} h"

incremental_dirs=0
# Another build reads and writes the current ones while it runs, so deleting them under it fails that build
# with missing dep-graph files. Only an idle target directory loses all of its incremental state.
if pgrep -x cargo >/dev/null 2>&1 || pgrep -x rustc >/dev/null 2>&1; then
  echo "[clean-stale] a cargo or rustc process is running; keeping current incremental directories"
  after="$(du -sk "$target_dir" 2>/dev/null | awk '{print $1}')"
  echo "[clean-stale] target: $((before / 1024)) MB -> $((${after:-0} / 1024)) MB"
  exit 0
fi
while IFS= read -r -d '' directory; do
  rm -rf -- "$directory"
  incremental_dirs=$((incremental_dirs + 1))
done < <(find "$target_dir" -type d -name incremental -prune -print0)

after="$(du -sk "$target_dir" 2>/dev/null | awk '{print $1}')"
after="${after:-0}"
echo "[clean-stale] removed $incremental_dirs incremental directories"
echo "[clean-stale] target: $((before / 1024)) MB -> $((after / 1024)) MB"
