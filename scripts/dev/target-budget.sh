#!/usr/bin/env bash
# Holds every Cargo target directory of this repository's worktrees to a hard size budget.
#
#   scripts/dev/target-budget.sh            enforce: TARGET_CAP_GB per target, TARGETS_TOTAL_GB across all
#   scripts/dev/target-budget.sh --report   sizes only, nothing removed
#
# Each worktree builds into its own target (a shared one makes cargo link one worktree's crates into another), so
# targets multiply with worktrees, and incremental state grows without bound in each. A target is busy while a
# build holds its profile's .cargo-lock open or rustc names it in --out-dir. Removing artifacts under a running
# build breaks its link step, so a busy target only loses incremental sessions nothing has written for
# BUSY_IDLE_MINUTES; an idle one over its cap loses its incremental state first and, if still over, everything,
# which costs one cold rebuild and nothing else.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cap_kb=$(( ${TARGET_CAP_GB:-8} * 1024 * 1024 ))
total_kb=$(( ${TARGETS_TOTAL_GB:-30} * 1024 * 1024 ))
busy_idle_minutes=${BUSY_IDLE_MINUTES:-30}
report_only=false
[ "${1:-}" = "--report" ] && report_only=true

targets=()
while IFS= read -r worktree; do
  [ -d "$worktree/target" ] && targets+=("$worktree/target")
done < <(git -C "$repo_root" worktree list --porcelain | awk '/^worktree /{sub(/^worktree /, ""); print}')
# Worktrees whose CARGO_TARGET_DIR sits beside them rather than inside, named <worktree>-target.
for dir in "$(dirname "$repo_root")"/sideseat*-target; do
  [ -d "$dir" ] && targets+=("$(cd "$dir" && pwd)")
done

size_kb() { du -sk "$1" 2>/dev/null | awk '{print $1}'; }
gb() { awk -v kb="$1" 'BEGIN { printf "%.1f GB", kb / 1048576 }'; }
# Busy also covers the gaps between a multi-step script's cargo runs, when nothing holds the lock: a worktree whose
# verify-head or push lock (<worktree>.lock) is taken, and a target anything has written in the last
# BUSY_IDLE_MINUTES.
busy() {
  local target="$1" worktree
  worktree="${target%/target}"
  worktree="${worktree%-target}"
  [ -d "$worktree.lock" ] && return 0
  lsof -t "$target"/*/.cargo-lock >/dev/null 2>&1 && return 0
  pgrep -f -- "$target/" >/dev/null 2>&1 && return 0
  [ -n "$(find "$target" -maxdepth 3 -mmin "-${busy_idle_minutes}" -print -quit 2>/dev/null)" ]
}

drop_incremental() {
  local target="$1" older_than="$2"
  while IFS= read -r -d '' session; do
    if [ -z "$older_than" ] || [ -z "$(find "$session" -type f -mmin "-$older_than" -print -quit 2>/dev/null)" ]; then
      rm -rf -- "$session"
    fi
  done < <(find "$target" -path '*/incremental/*' -mindepth 1 -maxdepth 4 -type d -prune -path '*/incremental/*' -print0 2>/dev/null)
}

enforce_cap() {
  local target="$1" before after
  before=$(size_kb "$target")
  [ "$before" -le "$cap_kb" ] && return 0
  if busy "$target"; then
    drop_incremental "$target" "$busy_idle_minutes"
    after=$(size_kb "$target")
    echo "[target-budget] $target: busy, $(gb "$before") -> $(gb "$after") (incremental idle ${busy_idle_minutes} min removed)"
    [ "$after" -gt "$cap_kb" ] && echo "[target-budget] $target: still over $(gb "$cap_kb") while a build runs; enforced when it finishes"
    return 0
  fi
  drop_incremental "$target" ""
  after=$(size_kb "$target")
  if [ "$after" -gt "$cap_kb" ]; then
    rm -rf -- "$target"
    after=0
  fi
  echo "[target-budget] $target: idle, $(gb "$before") -> $(gb "$after")"
}

total=0
for target in "${targets[@]}"; do
  kb=$(size_kb "$target")
  total=$((total + kb))
  state=idle
  busy "$target" && state=busy
  printf '[target-budget] %8s  %-4s  %s\n' "$(gb "$kb")" "$state" "$target"
done
echo "[target-budget] total $(gb "$total") of $(gb "$total_kb"); per target $(gb "$cap_kb")"
$report_only && exit 0

for target in "${targets[@]}"; do enforce_cap "$target"; done

# Over the total even with every target under its cap: clear idle targets, largest first, until under.
total=0
for target in "${targets[@]}"; do [ -d "$target" ] && total=$((total + $(size_kb "$target"))); done
if [ "$total" -gt "$total_kb" ]; then
  while IFS=$'\t' read -r kb target; do
    [ "$total" -le "$total_kb" ] && break
    busy "$target" && continue
    rm -rf -- "$target"
    total=$((total - kb))
    echo "[target-budget] $target: idle, removed $(gb "$kb") to bring the total under $(gb "$total_kb")"
  done < <(for target in "${targets[@]}"; do [ -d "$target" ] && printf '%s\t%s\n' "$(size_kb "$target")" "$target"; done | sort -rn)
fi
[ "$total" -gt "$total_kb" ] && echo "[target-budget] total $(gb "$total") still over $(gb "$total_kb"): only busy targets remain"
exit 0
