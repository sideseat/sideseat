#!/usr/bin/env bash
#
# Every read the API and the ingest path make, over the embedded store grown to a million spans: each must
# complete within the production memory limit and its latency ceiling.
#
#   scripts/perf/bench-reads.sh
#
# The corpus store is built the way `make footprint-storage` builds it - every captured export posted to a fresh
# release server - and the read harness (`read_paths` in the DuckDB adapter) copies it, folds its projects into
# one, so that one tenant holds the whole store and its sessions, replicates its rows under fresh identities until
# it holds `BENCH_READS_SPANS` spans, and runs each read nine times after a warm-up. The
# ceilings are milliseconds per read in `scripts/perf/read-ceilings.json`, for the fastest run, since a loaded
# machine only ever adds time; a read with no ceiling fails the run as surely as a slow one.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SPANS="${BENCH_READS_SPANS:-1000000}"
CEILINGS="${BENCH_READS_CEILINGS:-$ROOT/scripts/perf/read-ceilings.json}"
LOG="$(mktemp)"
STORE=""

cleanup() {
  [ -z "$STORE" ] || rm -rf "$STORE"
  rm -f "$LOG"
}
trap cleanup EXIT

echo "[bench-reads] building the corpus store"
(cd "$ROOT" && uv run --locked --script scripts/perf/storage-footprint.py embedded --keep) >"$LOG" 2>&1 || {
  tail -20 "$LOG"
  echo "[bench-reads] FAIL: the corpus store did not build" >&2
  exit 1
}
STORE="$(sed -n 's/^\[storage\] data kept in //p' "$LOG" | tail -1)"
[ -n "$STORE" ] && [ -f "$STORE/duckdb/sideseat.duckdb" ] || {
  tail -20 "$LOG"
  echo "[bench-reads] FAIL: storage-footprint.py named no kept store" >&2
  exit 1
}

echo "[bench-reads] measuring every read at $SPANS spans"
cd "$ROOT/server"
SIDESEAT_READ_PATHS_STORE="$STORE" SIDESEAT_READ_PATHS_ONE_PROJECT=1 SIDESEAT_READ_PATHS_SPANS="$SPANS" \
  SIDESEAT_READ_PATHS_CEILINGS="$CEILINGS" \
  cargo test --locked --release -p sideseat-adapter-duckdb --lib read_paths -- --ignored --nocapture --test-threads=1
