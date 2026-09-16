#!/usr/bin/env bash
#
# The two footprint ceilings that need a running server: idle resident memory, and resident memory under
# steady ingest. The other two are stated on live allocated bytes and live in `server/tests/footprint.rs`.
#
#   scripts/footprint-gates.sh
#
# This **enforces** rather than reports, as `bench-http-latency.sh` does: the run exits non-zero when a ceiling
# is missed. A number nobody compares against a target can drift arbitrarily far from the promise while every
# run passes, which is the whole reason the ceilings are written down.
#
# Four things it is careful about, each because getting it wrong produces a figure that looks like evidence and
# is not:
#
#   * **RSS comes from the OS, not from the process.** The server does not report its own memory and asking it
#     to would mean shipping an endpoint for a benchmark. `ps -o rss=` is what an operator's monitoring sees.
#   * **Idle means quiesced, not "just started".** Startup parses the rules assets, opens DuckDB, loads the
#     pricing catalogue and starts its sweepers, so a sample taken during that measures the transient. The
#     acceptance condition is *stability across consecutive readings* rather than elapsed time, because the
#     startup work is asynchronous and a fixed wait is a guess about a machine.
#   * **Steady ingest is a median, not a maximum.** One peak is whatever the allocator happened to hold at
#     that instant. The ceiling is on the window's median, and the maximum is reported beside it, ungated -
#     the same split `bench-http-latency.sh` makes between p95 and p99.
#   * **A failed export is not load.** Every post checks its status and the run fails if the server stopped
#     accepting, because a server refusing traffic has excellent memory usage.
#
# The ceilings are declared in `sideseat_core::core::constants` and repeated here as literals, since bash
# cannot read Rust. `the_footprint_script_enforces_the_declared_ceilings` compares the two, so a ceiling
# loosened in one place fails the build.
set -euo pipefail

IDLE_RSS_CEILING_BYTES=104857600
INGEST_RSS_CEILING_BYTES=419430400
# The rate the ingest ceiling is *stated* at, reported rather than enforced. This script cannot make a host
# sustain 5 000 spans/s, and failing a memory gate because the load generator fell short would be a false
# statement about memory. So the achieved rate is printed with the verdict, and a pass under target says the
# ceiling was met under lighter load than it claims - which is a caveat on the result, not a hidden one.
TARGET_SPANS_PER_SECOND="${FOOTPRINT_SPANS_PER_SECOND:-5000}"

PORT="${FOOTPRINT_PORT:-5597}"
IDLE_SETTLE_SECS="${FOOTPRINT_IDLE_SETTLE_SECS:-15}"
INGEST_SECS="${FOOTPRINT_INGEST_SECS:-60}"
SAMPLE_INTERVAL_SECS="${FOOTPRINT_SAMPLE_INTERVAL_SECS:-1}"
# Consecutive readings within this much of each other count as settled. 2 MB, because that is smaller than any
# startup phase and larger than the noise of a sweeper waking up.
IDLE_STABLE_DELTA_BYTES="${FOOTPRINT_IDLE_STABLE_DELTA_BYTES:-2097152}"
FIXTURE_NAME="${FOOTPRINT_FIXTURE:-langgraph/swarm}"
# Concurrent posters. One is not steady ingest: a single sequential poster idles between requests, and the
# resident figure would then describe a server at a fraction of the target rate.
LOADERS="${FOOTPRINT_LOADERS:-4}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
SERVER_PID=""

cleanup() {
  # This exact process, never `pkill -f sideseat`: running this beside a developer's own no-auth server would
  # otherwise kill theirs too.
  if [ -n "$SERVER_PID" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

fail() { echo "[footprint] FAIL: $*" >&2; exit 1; }
mb() { awk -v b="$1" 'BEGIN { printf "%.1f", b / 1048576 }'; }

rss_bytes() {
  local kb
  kb="$(ps -o rss= -p "$SERVER_PID" 2>/dev/null | tr -d ' ')"
  [ -n "$kb" ] || fail "the server is gone; nothing left to measure. Log: $(tail -5 "$WORK/server.log")"
  echo $((kb * 1024))
}

FIXTURE="$ROOT/server/tests/fixtures/messages/$FIXTURE_NAME"
[ -d "$FIXTURE" ] || fail "fixture $FIXTURE_NAME not found; capture it with scripts/message-fixtures/capture.sh"
ls "$FIXTURE"/*.pb >/dev/null 2>&1 || fail "fixture $FIXTURE_NAME holds no captured requests"

# Release, always: a debug build's footprint describes the debug build, and the ceilings are stated on what
# ships.
echo "[footprint] building"
(cd "$ROOT" && cargo build --locked --release -q -p sideseat-server)

# `env -i` with an explicit environment, for the same reason `bench-http-latency.sh` does it: an inherited
# `SIDESEAT_*` variable or a `~/.sideseat/sideseat.json` would silently change the backend, the retention or
# the caches, and a footprint figure would then come from a configuration nobody recorded. `exec`, so `$!` is
# the server's own pid rather than a shell that exits immediately - without it `cleanup` kills nothing and
# `ps -o rss=` reads a dead process.
echo "[footprint] starting server on :$PORT"
(cd "$WORK" && exec env -i \
  PATH="$PATH" HOME="$WORK" \
  SIDESEAT_DATA_DIR="$WORK" SIDESEAT_SECRETS_BACKEND=file \
  SIDESEAT_PORT="$PORT" SIDESEAT_UI_PORT="$((PORT + 1))" \
  SIDESEAT_OTEL_GRPC_PORT="$((PORT + 2))" \
  "$ROOT/target/release/sideseat" --no-auth > "$WORK/server.log" 2>&1) &
SERVER_PID=$!
for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:$PORT/api/v1/health" >/dev/null && break
  sleep 1
done
curl -sf "http://127.0.0.1:$PORT/api/v1/health" >/dev/null \
  || { echo "[footprint] server did not come up"; cat "$WORK/server.log"; exit 1; }

# --- gate 1: idle -----------------------------------------------------------
echo "[footprint] gate 1: idle resident memory (ceiling $(mb $IDLE_RSS_CEILING_BYTES) MB)"
sleep "$IDLE_SETTLE_SECS"
IDLE_RSS=0
STABLE=0
PREV=0
for _ in $(seq 1 60); do
  CURRENT="$(rss_bytes)"
  if [ "$PREV" -gt 0 ]; then
    DELTA=$(( CURRENT > PREV ? CURRENT - PREV : PREV - CURRENT ))
    if [ "$DELTA" -lt "$IDLE_STABLE_DELTA_BYTES" ]; then
      STABLE=$((STABLE + 1))
    else
      STABLE=0
    fi
  fi
  PREV="$CURRENT"
  # Three consecutive stable readings, not one: a single small delta happens in the middle of a phase.
  if [ "$STABLE" -ge 3 ]; then IDLE_RSS="$CURRENT"; break; fi
  sleep "$SAMPLE_INTERVAL_SECS"
done
[ "$IDLE_RSS" -gt 0 ] || fail "idle RSS never stabilised; last reading $(mb "$PREV") MB"
echo "[footprint] idle RSS: $(mb "$IDLE_RSS") MB"

# --- how many spans one pass of the fixture carries -------------------------
#
# Needed to report the achieved span rate, and it has to be measured rather than assumed: the requests of one
# fixture carry very different span counts. One full pass first, then the store's own count - which is also
# why the ingest loop below measures a *steady* write path, since ingestion is idempotent by span id and
# re-posting rewrites rather than grows.
echo "[footprint] loading one pass of $FIXTURE_NAME to learn its span count"
REQUESTS=0
for f in "$FIXTURE"/*.pb; do
  status="$(curl -s -o /dev/null -w '%{http_code}' -X POST --data-binary @"$f" \
    -H 'Content-Type: application/x-protobuf' "http://127.0.0.1:$PORT/otel/default/v1/traces")"
  [ "$status" = "200" ] || fail "the priming pass returned $status; the fixture did not load"
  REQUESTS=$((REQUESTS + 1))
done
sleep 4
SPANS_PER_PASS="$(curl -sf "http://127.0.0.1:$PORT/api/v1/project/default/otel/sessions?limit=1" |
  python3 -c 'import sys,json; r=json.load(sys.stdin).get("data") or []; print(r[0]["span_count"] if r else 0)')"
[ "$SPANS_PER_PASS" -gt 0 ] || fail "the fixture created no session, so its span count is unknown"
echo "[footprint] one pass = $REQUESTS requests / $SPANS_PER_PASS spans"

# --- gate 2: steady ingest --------------------------------------------------
#
# Load and sampling run concurrently: a sample taken between requests is not a reading of the ingest state.
echo "[footprint] gate 2: resident memory under steady ingest (ceiling $(mb $INGEST_RSS_CEILING_BYTES) MB)"
STOP_FILE="$WORK/stop"
rm -f "$STOP_FILE" "$WORK/post-errors"
: >"$WORK/posted"

for loader in $(seq 1 "$LOADERS"); do
  (
    while [ ! -f "$STOP_FILE" ]; do
      for f in "$FIXTURE"/*.pb; do
        [ -f "$STOP_FILE" ] && break
        status="$(curl -s -o /dev/null -w '%{http_code}' -X POST --data-binary @"$f" \
          -H 'Content-Type: application/x-protobuf' "http://127.0.0.1:$PORT/otel/default/v1/traces")"
        if [ "$status" != "200" ]; then
          echo "loader $loader got $status" >>"$WORK/post-errors"
          exit 0
        fi
        echo x >>"$WORK/posted"
      done
    done
  ) &
done

SAMPLES=()
START="$(date +%s)"
while [ $(( $(date +%s) - START )) -lt "$INGEST_SECS" ]; do
  SAMPLES+=("$(rss_bytes)")
  sleep "$SAMPLE_INTERVAL_SECS"
done
ELAPSED=$(( $(date +%s) - START ))
touch "$STOP_FILE"
wait

# A 503 here is `BufferFull` or a rate limit, which is the server protecting itself - a legitimate answer, and
# not load. Reported as a failure of the *measurement*, because the resident figure taken while the server was
# refusing describes a server doing less work than the ceiling claims.
if [ -s "$WORK/post-errors" ]; then
  fail "the server stopped accepting exports, so the samples do not describe steady ingest: $(sort -u "$WORK/post-errors" | head -3)"
fi
[ "${#SAMPLES[@]}" -gt 0 ] || fail "no samples taken"

MEDIAN_RSS="$(printf '%s\n' "${SAMPLES[@]}" | sort -n | awk '{ a[NR] = $1 } END { print a[int((NR + 1) / 2)] }')"
MAX_RSS="$(printf '%s\n' "${SAMPLES[@]}" | sort -n | tail -1)"
POSTED="$(wc -l <"$WORK/posted" | tr -d ' ')"
# Average spans per request across the fixture: the requests are not uniform, so this is an average by
# construction and is reported as an achieved rate rather than asserted as one.
ACHIEVED="$(awk -v posted="$POSTED" -v spans="$SPANS_PER_PASS" -v reqs="$REQUESTS" -v secs="$ELAPSED" \
  'BEGIN { if (secs > 0 && reqs > 0) printf "%.0f", posted * (spans / reqs) / secs; else print 0 }')"

echo "[footprint] steady ingest RSS: median $(mb "$MEDIAN_RSS") MB, max $(mb "$MAX_RSS") MB (ungated), ${#SAMPLES[@]} samples over ${ELAPSED}s"
echo "[footprint] achieved ~$ACHIEVED spans/s from $POSTED requests across $LOADERS loaders (ceiling is stated at $TARGET_SPANS_PER_SECOND spans/s)"
if [ "$ACHIEVED" -lt "$TARGET_SPANS_PER_SECOND" ]; then
  echo "[footprint] NOTE: the achieved rate is below the rate the ceiling is stated at, so a pass here means the ceiling held under lighter load than it claims"
fi

# --- verdict ----------------------------------------------------------------
FAILURES=0
if [ "$IDLE_RSS" -gt "$IDLE_RSS_CEILING_BYTES" ]; then
  echo "[footprint] FAIL: idle RSS $(mb "$IDLE_RSS") MB exceeds $(mb $IDLE_RSS_CEILING_BYTES) MB" >&2
  FAILURES=$((FAILURES + 1))
fi
if [ "$MEDIAN_RSS" -gt "$INGEST_RSS_CEILING_BYTES" ]; then
  echo "[footprint] FAIL: steady ingest median RSS $(mb "$MEDIAN_RSS") MB exceeds $(mb $INGEST_RSS_CEILING_BYTES) MB" >&2
  FAILURES=$((FAILURES + 1))
fi
[ "$FAILURES" -eq 0 ] || exit 1
echo "[footprint] both resident ceilings met"
