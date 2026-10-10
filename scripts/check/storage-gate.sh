#!/usr/bin/env bash
# The deterministic storage gate: the pinned corpus replayed in process (server/tests/storage_gate.rs), its
# stores measured per table, and the figures held to scripts/perf/storage-gate-baseline.json exactly.
#
#   scripts/check/storage-gate.sh                measure and compare
#   scripts/check/storage-gate.sh --update       measure and write the baseline (in the commit that moves it)
#   scripts/check/storage-gate.sh --equivalence  prove the figures do not depend on where or how it runs: the
#                                                RAM-disk replay, one on the ordinary disk and one of the
#                                                release build must leave one segment layout, and one on four
#                                                engine threads the same figures and answers (opt-in, slow)
#
# The replay writes every export durably, as the server does, and on a disk each sync costs milliseconds; on a
# RAM disk the run is bound by CPU instead. Where none can be had - no /dev/shm or no room in it, too little free
# memory, a failed attach - the same replay runs on the ordinary temp directory: slower, same figures. PASSES is
# fixed, because the baseline means something for one value only.
set -euo pipefail

PASSES=3
# The stores of three passes take about 80 MB; the volume leaves room for the copy the index measurement makes.
VOLUME_MB=512
# Free memory the RAM disk needs before it is made: the volume, and a margin for the build and the replay.
NEEDED_MB=$((VOLUME_MB * 3))

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

update=""
equivalence=""
case "${1:-}" in
  --update) update="--update-baseline" ;;
  --equivalence) equivalence=1 ;;
  "") ;;
  *) echo "usage: $0 [--update | --equivalence]" >&2; exit 2 ;;
esac

name="sideseat-gate-$$"
volume=""
device=""
work=""
cleanup() {
  if [ -n "$device" ]; then
    hdiutil detach "$device" -force >/dev/null 2>&1 || true
  fi
  if [ -n "$work" ] && [ -z "$device" ]; then
    rm -rf "$work"
  fi
}
# Cleanup runs once, on exit; a signal exits, with the status a shell gives it, and so runs it.
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# A volume a killed run left behind is detached here, matched by its unique name: the pid that made it is gone.
sweep_stale() {
  case "$(uname -s)" in
    Darwin)
      for stale in /Volumes/sideseat-gate-*; do
        [ -d "$stale" ] || continue
        pid="${stale##*-}"
        if ! kill -0 "$pid" 2>/dev/null; then
          hdiutil detach "$stale" -force >/dev/null 2>&1 || true
        fi
      done
      ;;
    Linux)
      for stale in /dev/shm/sideseat-gate-*; do
        [ -d "$stale" ] || continue
        pid="${stale##*-}"
        pid="${pid%%.*}"
        if ! kill -0 "$pid" 2>/dev/null; then
          rm -rf "$stale"
        fi
      done
      ;;
  esac
}

free_mb() {
  case "$(uname -s)" in
    Darwin)
      local page free inactive
      page="$(sysctl -n hw.pagesize)"
      free="$(vm_stat | awk '/Pages free/ {gsub("\\.", "", $3); print $3}')"
      inactive="$(vm_stat | awk '/Pages inactive/ {gsub("\\.", "", $3); print $3}')"
      echo $(((free + inactive) * page / 1048576))
      ;;
    Linux)
      awk '/MemAvailable/ {print int($2 / 1024)}' /proc/meminfo
      ;;
    *) echo 0 ;;
  esac
}

sweep_stale
if [ "$(free_mb)" -ge "$NEEDED_MB" ]; then
  case "$(uname -s)" in
    Darwin)
      if device="$(hdiutil attach -nomount "ram://$((VOLUME_MB * 2048))" 2>/dev/null | awk '{print $1}')" \
        && [ -n "$device" ] && diskutil erasevolume APFS "$name" "$device" >/dev/null 2>&1; then
        volume="/Volumes/$name"
        work="$volume"
      else
        if [ -n "$device" ]; then
          hdiutil detach "$device" -force >/dev/null 2>&1 || true
        fi
        device=""
      fi
      ;;
    Linux)
      # A tmpfs is often far smaller than the memory behind it: the stores must fit in the space it has left.
      if [ -d /dev/shm ] && [ -w /dev/shm ] \
        && [ "$(df -Pm /dev/shm | awk 'NR == 2 {print $4}')" -ge "$VOLUME_MB" ]; then
        work="$(mktemp -d "/dev/shm/$name.XXXXXX")"
      fi
      ;;
  esac
fi
if [ -z "$work" ]; then
  echo "[storage-gate] no RAM disk; replaying on the temp directory, which is slower and measures the same"
  work="$(mktemp -d "${TMPDIR:-/tmp}/$name.XXXXXX")"
fi

# replay DIR [cargo profile flag] [DuckDB threads]: the corpus, PASSES times, into DIR.
replay() {
  local log
  log="$(mktemp "${TMPDIR:-/tmp}/$name-log.XXXXXX")"
  if ! (cd server && SIDESEAT_STORAGE_GATE_DIR="$1" SIDESEAT_STORAGE_GATE_PASSES="$PASSES" \
    SIDESEAT_STORAGE_GATE_THREADS="${3:-1}" \
    cargo test --locked -q ${2:+"$2"} -p sideseat-server --test storage_gate -- --ignored --exact \
    the_pinned_corpus_replays_into_the_same_stores --nocapture) >"$log" 2>&1; then
    tail -30 "$log" >&2
    rm -f "$log"
    echo "[storage-gate] FAIL: the replay did not finish" >&2
    exit 1
  fi
  grep -E "^\[storage-gate\]" "$log" || true
  rm -f "$log"
}

started=$SECONDS
replay "$work/store"
replayed=$SECONDS
if [ -n "$equivalence" ]; then
  disk="$(mktemp -d "${TMPDIR:-/tmp}/$name-disk.XXXXXX")"
  trap 'rm -rf "$disk"; cleanup' EXIT
  replay "$disk/store"
  uv run --locked --script scripts/perf/storage-footprint.py embedded --store "$work/store" --same-layout "$disk/store"
  rm -rf "$disk/store"
  replay "$disk/store" --release
  uv run --locked --script scripts/perf/storage-footprint.py embedded --store "$work/store" --same-layout "$disk/store"
  # Four engine threads, the server's most: the same figures, though DuckDB places some segments elsewhere.
  rm -rf "$disk/store"
  replay "$disk/store" "" 4
  uv run --locked --script scripts/perf/storage-footprint.py embedded --store "$work/store" --same-figures "$disk/store"
  # And the same answers: every read bench-reads makes and the message views, byte for byte.
  uv run --locked --script scripts/perf/storage-footprint.py embedded --store "$work/store" --same-answers "$disk/store"
  echo "[storage-gate] the RAM disk, the ordinary disk and the release build leave one layout, and four engine" \
    "threads the same figures and answers ($((SECONDS - started))s)"
  exit 0
fi
uv run --locked --script scripts/perf/storage-footprint.py embedded --store "$work/store" --passes "$PASSES" \
  --baseline scripts/perf/storage-gate-baseline.json $update
# The gate's time, by phase, so its growth shows: it runs in every `make check`.
echo "[storage-gate] build and replay $((replayed - started))s, measurement $((SECONDS - replayed))s, total $((SECONDS - started))s"
