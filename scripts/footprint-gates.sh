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
# The ceilings are declared in `sideseat_core::constants` and repeated here as literals, since bash
# cannot read Rust. `the_footprint_script_enforces_the_declared_ceilings` compares the two, so a ceiling
# loosened in one place fails the build.
set -euo pipefail

IDLE_RSS_CEILING_BYTES=104857600
INGEST_RSS_CEILING_BYTES=419430400
# The rate the ingest ceiling is stated at, and it is **enforced against a floor**, not merely printed.
#
# Printing it was wrong: a resident figure taken at 200 spans/s says nothing about whether 5 000 spans/s stays
# under 400 MB, so reporting that as a pass is a gate that sees less than it claims. A floor rather than the
# exact target, because a load generator built from `curl` in a loop will not reach 5 000 spans/s on every host
# and demanding it exactly would make the gate unrunnable rather than strict.
TARGET_SPANS_PER_SECOND="${FOOTPRINT_SPANS_PER_SECOND:-5000}"

PORT="${FOOTPRINT_PORT:-5597}"
IDLE_SETTLE_SECS="${FOOTPRINT_IDLE_SETTLE_SECS:-15}"
INGEST_SECS="${FOOTPRINT_INGEST_SECS:-60}"
SAMPLE_INTERVAL_SECS="${FOOTPRINT_SAMPLE_INTERVAL_SECS:-1}"
# Consecutive readings within this much of each other count as settled. 2 MB, because that is smaller than any
# startup phase and larger than the noise of a sweeper waking up.
IDLE_STABLE_DELTA_BYTES="${FOOTPRINT_IDLE_STABLE_DELTA_BYTES:-2097152}"
# This gate measures resident memory at a stated *rate*. The golden `langgraph/swarm` fixture remains the
# correctness, latency and queued-byte workload, but its 1.7 MB of semantic history makes CPU extraction the
# limiter at a few hundred spans/s even with dozens of clients. That cannot exercise a 5 000 spans/s memory
# ceiling. Generate a compact, valid OTLP request with many minimal spans so this gate varies rate rather than
# prompt complexity. `FOOTPRINT_RATE_FIXTURE` may still select a captured fixture for diagnostics.
FIXTURE_NAME="${FOOTPRINT_RATE_FIXTURE:-synthetic/minimal-500}"
# Concurrent posters. One is not steady ingest: a single sequential poster idles between requests, and the
# resident figure would then describe a server at a fraction of the target rate.
LOADERS="${FOOTPRINT_LOADERS:-4}"
# A per-request ceiling for the load generator. Without one a stalled response blocks its loader forever, and
# waiting for that pid is the same hang a bare `wait` produced. Generous against the large-export p99 this
# repository documents, so it fires on a stall rather than on a slow write.
LOADER_TIMEOUT_SECS="${FOOTPRINT_LOADER_TIMEOUT_SECS:-30}"
# The same ceiling for every other request the script makes. Short, because these are health checks and a single
# fixture post rather than sustained load.
REQUEST_TIMEOUT_SECS="${FOOTPRINT_REQUEST_TIMEOUT_SECS:-30}"
# How long the server gets to exit on SIGTERM before SIGKILL. See `cleanup`.
SHUTDOWN_GRACE_SECS="${FOOTPRINT_SHUTDOWN_GRACE_SECS:-15}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
if [[ "$CARGO_TARGET_DIR" != /* ]]; then
  CARGO_TARGET_DIR="$ROOT/$CARGO_TARGET_DIR"
fi
export CARGO_TARGET_DIR
WORK="$(mktemp -d)"
SERVER_PID=""

cleanup() {
  # This exact process, never `pkill -f sideseat`: running this beside a developer's own no-auth server would
  # otherwise kill theirs too.
  #
  # And the wait is **bounded**, then escalated. A bare `wait` here was the last place the script could hang: every
  # curl is timed out now, but a server whose graceful shutdown deadlocks - draining a topic, say - would hold the
  # trap open forever, and a run that never exits is indistinguishable from one still working.
  if [ -n "$SERVER_PID" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    for _ in $(seq 1 "$SHUTDOWN_GRACE_SECS"); do
      kill -0 "$SERVER_PID" 2>/dev/null || break
      sleep 1
    done
    # Still there: SIGKILL, which no handler can defer. The exit status is discarded because by this point the
    # measurement is over and the only remaining job is not to leave a process holding these ports.
    if kill -0 "$SERVER_PID" 2>/dev/null; then
      echo "[footprint] the server did not exit in ${SHUTDOWN_GRACE_SECS}s; killing it" >&2
      kill -9 "$SERVER_PID" 2>/dev/null || true
    fi
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

fail() { echo "[footprint] FAIL: $*" >&2; exit 1; }
mb() { awk -v b="$1" 'BEGIN { printf "%.1f", b / 1048576 }'; }

# Every curl in this script goes through one of these, so "add a timeout" cannot be forgotten at a new call site.
#
# Only the load loop had `--max-time`, and the other four calls - health, the readiness poll, the priming pass and
# the session-count read - could each block forever on a server that accepts the connection and never answers.
# The run then never reaches its verdict, which looks like a slow machine rather than a hang.
curl_q() { curl -s --max-time "$REQUEST_TIMEOUT_SECS" "$@"; }
curl_f() { curl -sf --max-time "$REQUEST_TIMEOUT_SECS" "$@"; }

rss_bytes() {
  local kb
  kb="$(ps -o rss= -p "$SERVER_PID" 2>/dev/null | tr -d ' ')"
  [ -n "$kb" ] || fail "the server is gone; nothing left to measure. Log: $(tail -5 "$WORK/server.log")"
  echo $((kb * 1024))
}

KNOWN_SPANS_PER_PASS=""
if [ "$FIXTURE_NAME" = "synthetic/minimal-500" ]; then
  FIXTURE="$WORK/rate-fixture"
  mkdir -p "$FIXTURE"
  KNOWN_SPANS_PER_PASS=500
  python3 - "$FIXTURE/req-001.pb" "$KNOWN_SPANS_PER_PASS" <<'PY'
import struct
import sys

path = sys.argv[1]
count = int(sys.argv[2])

def varint(value):
    out = bytearray()
    while value >= 0x80:
        out.append((value & 0x7f) | 0x80)
        value >>= 7
    out.append(value)
    return bytes(out)

def key(field, wire):
    return varint((field << 3) | wire)

def bytes_field(field, value):
    return key(field, 2) + varint(len(value)) + value

def fixed64_field(field, value):
    return key(field, 1) + struct.pack("<Q", value)

start = 1_787_825_000_000_000_000
spans = bytearray()
for ordinal in range(count):
    identity = ordinal + 1
    trace_id = b"footprnt" + identity.to_bytes(8, "big")
    span_id = identity.to_bytes(8, "big")
    span = (
        bytes_field(1, trace_id)
        + bytes_field(2, span_id)
        + bytes_field(5, b"footprint-rate")
        + key(6, 0) + varint(1)
        + fixed64_field(7, start + ordinal)
        + fixed64_field(8, start + ordinal + 1_000_000)
    )
    spans.extend(bytes_field(2, span))

scope_spans = bytes(spans)
resource_spans = bytes_field(2, scope_spans)
request = bytes_field(1, resource_spans)
with open(path, "wb") as output:
    output.write(request)
PY
else
  FIXTURE="$ROOT/server/tests/fixtures/messages/$FIXTURE_NAME"
  [ -d "$FIXTURE" ] || fail "fixture $FIXTURE_NAME not found; capture it with scripts/message-fixtures/capture.sh"
  ls "$FIXTURE"/*.pb >/dev/null 2>&1 || fail "fixture $FIXTURE_NAME holds no captured requests"
fi

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
  "$CARGO_TARGET_DIR/release/sideseat" --no-auth > "$WORK/server.log" 2>&1) &
SERVER_PID=$!
for _ in $(seq 1 60); do
  curl_f "http://127.0.0.1:$PORT/api/v1/health" >/dev/null && break
  sleep 1
done
curl_f "http://127.0.0.1:$PORT/api/v1/health" >/dev/null \
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
  status="$(curl_q -o /dev/null -w '%{http_code}' -X POST --data-binary @"$f" \
    -H 'Content-Type: application/x-protobuf' "http://127.0.0.1:$PORT/otel/default/v1/traces")"
  [ "$status" = "200" ] || fail "the priming pass returned $status; the fixture did not load"
  REQUESTS=$((REQUESTS + 1))
done
sleep 4
if [ -n "$KNOWN_SPANS_PER_PASS" ]; then
  SPANS_PER_PASS="$KNOWN_SPANS_PER_PASS"
else
  SPANS_PER_PASS="$(curl_f "http://127.0.0.1:$PORT/api/v1/project/default/otel/sessions?limit=1" |
    python3 -c 'import sys,json; r=json.load(sys.stdin).get("data") or []; print(r[0]["span_count"] if r else 0)')"
fi
[ "$SPANS_PER_PASS" -gt 0 ] || fail "the fixture created no session, so its span count is unknown"
echo "[footprint] one pass = $REQUESTS requests / $SPANS_PER_PASS spans"

# --- gate 2: steady ingest --------------------------------------------------
#
# Load and sampling run concurrently: a sample taken between requests is not a reading of the ingest state.
# Pace each loader toward the rate the ceiling actually names. An unbounded generator made a fast host run at
# 8 000+ spans/s and then compared that RSS with the 5 000 spans/s ceiling; that is a different workload in the
# opposite direction from the low-rate false pass guarded below. The 0.75 factor leaves room for request latency,
# while the mandatory 90% achieved-rate floor still rejects a host that does not reach the stated regime.
LOADER_PACE_SECS="$(awk -v loaders="$LOADERS" -v spans="$SPANS_PER_PASS" -v reqs="$REQUESTS" \
  -v target="$TARGET_SPANS_PER_SECOND" \
  'BEGIN { if (target > 0 && reqs > 0) printf "%.6f", 0.75 * loaders * spans / (reqs * target); else print 0 }')"
echo "[footprint] gate 2: resident memory under steady ingest (ceiling $(mb $INGEST_RSS_CEILING_BYTES) MB)"
STOP_FILE="$WORK/stop"
rm -f "$STOP_FILE" "$WORK/post-errors"
: >"$WORK/posted"

LOADER_PIDS=()
for loader in $(seq 1 "$LOADERS"); do
  (
    while [ ! -f "$STOP_FILE" ]; do
      for f in "$FIXTURE"/*.pb; do
        [ -f "$STOP_FILE" ] && break
        # `--max-time`, or a stalled response blocks this loader forever and the pid wait below never returns -
        # the same hang the bare `wait` produced, reached from the other side.
        #
        # `|| true` on the assignment, and the curl exit status captured separately: without it a transport
        # error (exit 7, 28, ...) trips `set -e` and kills this subshell *before* it records anything, so the
        # verdict below reads a clean `post-errors` and reports a pass for a run whose load stopped early.
        # Reset per iteration. `|| curl_status=$?` only assigns on failure, so without this a success carries
        # the previous iteration's value - harmless today because the loop exits on the first failure, and one
        # edit away from a loader that reports a stale error or hides a real one.
        curl_status=0
        status="$(curl -s --max-time "$LOADER_TIMEOUT_SECS" -o /dev/null -w '%{http_code}' \
          -X POST --data-binary @"$f" -H 'Content-Type: application/x-protobuf' \
          "http://127.0.0.1:$PORT/otel/default/v1/traces")" || curl_status=$?
        if [ "$curl_status" != "0" ]; then
          echo "loader $loader: curl failed with exit ${curl_status}" >>"$WORK/post-errors"
          exit 0
        fi
        if [ "$status" != "200" ]; then
          echo "loader $loader got HTTP $status" >>"$WORK/post-errors"
          exit 0
        fi
        echo x >>"$WORK/posted"
        sleep "$LOADER_PACE_SECS"
      done
    done
  ) &
  LOADER_PIDS+=("$!")
done

SAMPLES=()
START="$(date +%s)"
while [ $(( $(date +%s) - START )) -lt "$INGEST_SECS" ]; do
  SAMPLES+=("$(rss_bytes)")
  sleep "$SAMPLE_INTERVAL_SECS"
done
ELAPSED=$(( $(date +%s) - START ))
touch "$STOP_FILE"
# The loaders **by pid**, never a bare `wait`.
#
# A bare `wait` waits for every background child, and the server is one of them - so it blocked until the
# server exited, which only the EXIT trap does, and the trap cannot run while `wait` is blocked. The run hung
# here forever and never evaluated a single ceiling. A gate that cannot reach its own verdict is worse than no
# gate: it looks like a slow machine.
for pid in "${LOADER_PIDS[@]}"; do
  wait "$pid" 2>/dev/null || true
done

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
# Below the rate the ceiling is stated at, the resident figure describes a different workload - so this is a
# failed *measurement*, not a passed gate. A run achieving 200 spans/s at 350 MB says nothing about whether
# 5 000 spans/s stays under 400 MB, and reporting it as a pass is the "gate that sees less than it claims"
# shape this file exists to avoid.
#
# A floor just below the target rather than the exact figure, because a `curl`-in-a-loop generator will not hit
# 5 000 spans/s to the request on every host. 90%, not 50%: at half the rate the measurement describes a
# materially different workload, and a system at 300 MB under 2 500 spans/s can be at 500 MB under 5 000 - so a
# 50% floor was the same false pass in a smaller size.
# `FOOTPRINT_MIN_RATE_FRACTION` is what an operator lowers deliberately, which leaves a record in the command
# rather than in a note nobody reads.
# The fraction is validated before it is used. `awk` happily emits `nan` for a non-numeric one, and the integer
# comparison below then errors *inside* an `if`, where `set -e` does not terminate the script - so the gate would
# be skipped rather than failed, which is the shape this whole file exists to remove.
RATE_FRACTION="${FOOTPRINT_MIN_RATE_FRACTION:-0.9}"
# The pattern alone was not enough: `.` contains only permitted characters, and `awk` reads it as zero - so
# `MIN_RATE` became 0 and every achieved rate passed a gate that says it enforces 90%. A validator that admits a
# value which disables the thing it guards is the same defect as no validator.
case "$RATE_FRACTION" in
  ''|.|*[!0-9.]*|*.*.*) fail "FOOTPRINT_MIN_RATE_FRACTION must be a number, got '$RATE_FRACTION'" ;;
esac
MIN_RATE="$(awk -v target="$TARGET_SPANS_PER_SECOND" -v frac="$RATE_FRACTION" \
  'BEGIN { printf "%.0f", target * frac }')"
case "$MIN_RATE" in
  ''|*[!0-9]*) fail "could not compute a rate floor from target=$TARGET_SPANS_PER_SECOND fraction=$RATE_FRACTION" ;;
esac
# And a floor of zero is no floor, however it was arrived at. Refused rather than reported, because a gate that
# admits everything while claiming a threshold is worse than an absent gate.
[ "$MIN_RATE" -gt 0 ] || fail "the computed rate floor is 0, which enforces nothing (target=$TARGET_SPANS_PER_SECOND fraction=$RATE_FRACTION)"
if [ "$ACHIEVED" -lt "$MIN_RATE" ]; then
  fail "achieved ~$ACHIEVED spans/s, below the $MIN_RATE floor for a ceiling stated at $TARGET_SPANS_PER_SECOND spans/s. The resident figure describes a lighter workload than the ceiling claims, so it is not evidence about the ceiling. Raise the load (FOOTPRINT_LOADERS) or lower the floor deliberately (FOOTPRINT_MIN_RATE_FRACTION)."
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
