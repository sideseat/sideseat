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

# A running build reads and rewrites fingerprints and artifacts that cargo-sweep judges by timestamp, so sweeping
# under it deletes files the build is about to write beside (`failed to write .../invoked.timestamp`).
# Busy means busy for this target: a build holds its profile directory's .cargo-lock open while it runs, and rustc
# names the target in its --out-dir. Several worktrees build into targets of their own, so a build anywhere on the
# machine says nothing about this one. Without lsof every cargo or rustc counts, which keeps more, never less.
build_running=false
if command -v lsof >/dev/null 2>&1; then
  if lsof -t "$target_dir"/*/.cargo-lock >/dev/null 2>&1 || pgrep -f -- "$target_dir/" >/dev/null 2>&1; then
    build_running=true
  fi
elif pgrep -x cargo >/dev/null 2>&1 || pgrep -x rustc >/dev/null 2>&1; then
  build_running=true
fi

if [ "$build_running" = true ]; then
  # A dependency's artifacts are written once and then only read, and reading leaves its timestamp alone: a
  # day-old rlib can be the one the running build links next. Removing such units by age broke builds mid-link
  # ("can't find crate"), so a busy target keeps every unit.
  echo "[clean-stale] a build is using this target; keeping every unit"
elif command -v cargo-sweep >/dev/null 2>&1; then
  # Not `--installed`: it decides which artifacts belong to an installed toolchain by fingerprinting every
  # one, and a toolchain rustup cannot fingerprint (a missing manifest) made it delete the active
  # toolchain's release and profiling builds while they were being built.
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

# Every edit of a crate links a new test executable beside the old ones (hundreds per day for the goldens), and
# nothing reuses the old ones - except a build that finds its crate unchanged, which runs the executable it linked
# however long ago. So they go only from an idle target, when untouched for STALE_HOURS.
stale_executables=0
if [ "$build_running" = false ]; then
  stale_executables="$(find "$target_dir"/*/deps -maxdepth 1 -type f -perm -u+x ! -name '*.*' -mmin "+$stale_minutes" -print -delete 2>/dev/null | wc -l | tr -d ' ')"
fi
echo "[clean-stale] removed $stale_executables linked executables untouched for ${STALE_HOURS:-6} h"

incremental_dirs=0
# Another build reads and writes the current ones while it runs, so deleting them under it fails that build
# with missing dep-graph files. Only an idle target directory loses all of its incremental state.
if [ "$build_running" = true ]; then
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
