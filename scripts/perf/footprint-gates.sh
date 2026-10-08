#!/usr/bin/env bash
#
# The two footprint ceilings that need a running server - idle memory, and memory under steady ingest - and the
# cross-check that gives them their meaning: the same load inside a Linux container whose memory limit the kernel
# enforces. The other two ceilings are stated on live allocated bytes and live in `server/tests/footprint.rs`.
#
#   scripts/perf/footprint-gates.sh             # local: this host's release binary against the two ceilings
#   scripts/perf/footprint-gates.sh container   # the aarch64-linux image under enforced limits
#
# This **enforces** rather than reports, as `bench-http-latency.sh` does: the run exits non-zero when a ceiling
# is missed. A number nobody compares against a target can drift arbitrarily far from the promise while every
# run passes, which is the whole reason the ceilings are written down.
#
# Six things it is careful about, each because getting it wrong produces a figure that looks like evidence and
# is not:
#
#   * **The figure is the memory the OS charges, not RSS.** `phys_footprint` on macOS, and on Linux the
#     `memory.current` of a cgroup that holds the server alone, minus nothing. RSS also counts pages an allocator
#     has freed and marked reusable, which macOS takes back without asking, so it moves for reasons the program
#     does not control: on one steady-ingest run RSS climbed to 560 MB while the charged figure stayed between 215
#     and 325 MB. The charged figure is what a limit is enforced against. On Linux a page jemalloc has freed and
#     not purged is charged to the cgroup, so the binary states its purge policy (`runtime/allocation.rs`) rather
#     than leaving retained pages for the kernel to find.
#   * **RSS is reported beside it, ungated**, so a regression in retained pages stays visible.
#   * **An enforced limit is the final word.** A charged figure describes a process that was never refused memory.
#     `container` runs the image under the 400 MB ceiling as a hard limit, and under the 2 GB target host with
#     one core, and fails if the kernel kills the server or reclaiming slows it below its floor. Under a limit
#     the charged figure cannot exceed the limit, so there the gate is the server surviving at the rate, and
#     the figures are reported.
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
# The ceilings, rates and the target host are declared in `sideseat_core::constants` and repeated here as
# literals, since bash cannot read Rust. `the_footprint_script_enforces_the_declared_ceilings` compares the two,
# so a ceiling loosened in one place fails the build.
set -euo pipefail

IDLE_MEMORY_CEILING_BYTES=104857600
INGEST_MEMORY_CEILING_BYTES=419430400
# The rate the ingest ceiling is stated at, and it is **enforced against a floor**, not merely printed.
#
# Printing it was wrong: a memory figure taken at 200 spans/s says nothing about whether 5 000 spans/s stays
# under 400 MB, so reporting that as a pass is a gate that sees less than it claims. A floor rather than the
# exact target, because a load generator built from a few Python threads will not reach 5 000 spans/s on every
# host and demanding it exactly would make the gate unrunnable rather than strict.
INGEST_SPANS_PER_SECOND=5000
# The host the product is stated for: one core, 2 GB for the server and its embedded backend, 10 000 spans/s.
TARGET_HOST_MEMORY_BYTES=2147483648
TARGET_HOST_CORES=1
TARGET_HOST_SPANS_PER_SECOND=10000

MODE="${1:-local}"
case "$MODE" in
  local | container) ;;
  *) echo "usage: $0 [local|container]" >&2; exit 2 ;;
esac

TARGET_SPANS_PER_SECOND="${FOOTPRINT_SPANS_PER_SECOND:-$INGEST_SPANS_PER_SECOND}"
PORT="${FOOTPRINT_PORT:-5597}"
IDLE_SETTLE_SECS="${FOOTPRINT_IDLE_SETTLE_SECS:-15}"
INGEST_SECS="${FOOTPRINT_INGEST_SECS:-60}"
SAMPLE_INTERVAL_SECS="${FOOTPRINT_SAMPLE_INTERVAL_SECS:-1}"
# Consecutive readings within this much of each other count as settled. 2 MB, because that is smaller than any
# startup phase and larger than the noise of a sweeper waking up.
IDLE_STABLE_DELTA_BYTES="${FOOTPRINT_IDLE_STABLE_DELTA_BYTES:-2097152}"
# This gate measures memory at a stated *rate*. The golden `langgraph/swarm` fixture remains the correctness,
# latency and queued-byte workload, but its 1.7 MB of semantic history makes CPU extraction the limiter at a few
# hundred spans/s even with dozens of clients. That cannot exercise a 5 000 spans/s memory ceiling. Generate a
# compact, valid OTLP request with many minimal spans so this gate varies rate rather than prompt complexity.
# `FOOTPRINT_RATE_FIXTURE` may still select a captured fixture for diagnostics.
FIXTURE_NAME="${FOOTPRINT_RATE_FIXTURE:-synthetic/minimal-500}"
# Concurrent posters. One is not steady ingest: a single sequential poster idles between requests, and the
# memory figure would then describe a server at a fraction of the target rate.
LOADERS="${FOOTPRINT_LOADERS:-4}"
# A per-request ceiling keeps a stalled response from blocking its loader. It is generous relative to the
# documented large-export p99, so it distinguishes a stall from a slow write.
LOADER_TIMEOUT_SECS="${FOOTPRINT_LOADER_TIMEOUT_SECS:-30}"
# The same ceiling for every other request the script makes. Short, because these are health checks and a single
# fixture post rather than sustained load.
REQUEST_TIMEOUT_SECS="${FOOTPRINT_REQUEST_TIMEOUT_SECS:-30}"
# How long the server gets to exit on SIGTERM before SIGKILL. See `stop_server`.
SHUTDOWN_GRACE_SECS="${FOOTPRINT_SHUTDOWN_GRACE_SECS:-15}"

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CARGO_TARGET_DIR="$(bash "$ROOT/scripts/dev/cargo-target-dir.sh")"
export CARGO_TARGET_DIR
WORK="$(mktemp -d)"
OS="$(uname -s)"
SERVER_PID=""
CONTAINER=""
CGROUP_DIR=""

fail() { echo "[footprint] FAIL: $*" >&2; exit 1; }
mb() { awk -v b="$1" 'BEGIN { printf "%.1f", b / 1048576 }'; }
median() { printf '%s\n' "$@" | sort -n | awk '{ a[NR] = $1 } END { print a[int((NR + 1) / 2)] }'; }
largest() { printf '%s\n' "$@" | sort -n | tail -1; }

stop_server() {
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
    SERVER_PID=""
  fi
  if [ -n "$CONTAINER" ]; then
    docker rm -fv "$CONTAINER" >/dev/null 2>&1 || true
    CONTAINER=""
  fi
}
cleanup() {
  stop_server
  rm -rf "$WORK"
}
trap cleanup EXIT

# Every curl in this script goes through one of these, so "add a timeout" cannot be forgotten at a new call site.
#
# Only the load loop had `--max-time`, and the other four calls - health, the readiness poll, the priming pass and
# the session-count read - could each block forever on a server that accepts the connection and never answers.
# The run then never reaches its verdict, which looks like a slow machine rather than a hang.
curl_q() { curl -s --max-time "$REQUEST_TIMEOUT_SECS" "$@"; }
curl_f() { curl -sf --max-time "$REQUEST_TIMEOUT_SECS" "$@"; }

server_log() {
  if [ -n "$CONTAINER" ]; then docker logs --tail 5 "$CONTAINER" 2>&1; else tail -5 "$WORK/server.log"; fi
}

# One reading: the charged bytes, then RSS. Empty when the server is gone, which the caller judges: locally that
# ends the run, under a limit it is the verdict.
sample() {
  if [ -n "$CONTAINER" ]; then
    # One `docker exec` for both, so the two describe the same instant. The server is the container's pid 1: the
    # entrypoint `exec`s it.
    { docker exec "$CONTAINER" sh -c 'cat /sys/fs/cgroup/memory.current; grep "^VmRSS:" /proc/1/status' \
      2>/dev/null || true; } |
      awk 'NR == 1 { charged = $1 } /^VmRSS:/ { rss = $2 * 1024 } END { if (charged && rss) print charged, rss }'
    return 0
  fi
  local kb charged=""
  kb="$(ps -o rss= -p "$SERVER_PID" 2>/dev/null | tr -d ' ' || true)"
  [ -n "$kb" ] || return 0
  case "$OS" in
    Darwin)
      charged="$({ footprint --noCategories -f bytes -p "$SERVER_PID" 2>/dev/null || true; } |
        awk '/phys_footprint:/ { print $2; exit }')"
      ;;
    Linux) charged="$(cat "$CGROUP_DIR/memory.current" 2>/dev/null || true)" ;;
  esac
  if [ -n "$charged" ]; then echo "$charged $((kb * 1024))"; fi
}
sample_or_fail() {
  local reading
  reading="$(sample)"
  [ -n "$reading" ] || fail "the server is gone; nothing left to measure. Log: $(server_log)"
  echo "$reading"
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
  [ -d "$FIXTURE" ] || fail "fixture $FIXTURE_NAME not found; capture it with make capture P=<producer>"
  ls "$FIXTURE"/*.pb >/dev/null 2>&1 || fail "fixture $FIXTURE_NAME holds no captured requests"
fi

# The loaders send **new telemetry on every post**: each pass of each loader rewrites every trace and span id of
# the fixture - parents and links with the same mapping, so trees stay intact - under a key unique to that loader
# and pass. Re-posting the same bytes measured the redelivery path instead: after the priming pass every export
# was found already stored rather than written as new spans, and on one host that path ran at 1.9 times the rate
# of fresh ingest (6,141 against 3,276 spans/s with 500-span exports), so the achieved rate and the memory figure
# both described a lighter workload than steady ingest. The ids are rewritten in place by XOR, which keeps every
# length and so every other byte of the request.
cat >"$WORK/loaders.py" <<'LOADERS'
import http.client
import random
import sys
import threading
import time
from pathlib import Path

fixture, work, loaders, port, timeout, pace, stop = sys.argv[1:8]
loaders, port, timeout, pace = int(loaders), int(port), float(timeout), float(pace)
stop, work = Path(stop), Path(work)


def varint(data, at):
    value = shift = 0
    while True:
        byte = data[at]
        at += 1
        value |= (byte & 0x7F) << shift
        shift += 7
        if byte < 0x80:
            return value, at


def fields(data, start, end):
    """(field, start, end) of every length-delimited field in data[start:end]."""
    at = start
    while at < end:
        key, at = varint(data, at)
        wire = key & 7
        if wire == 0:
            _, at = varint(data, at)
        elif wire == 1:
            at += 8
        elif wire == 5:
            at += 4
        elif wire == 2:
            length, at = varint(data, at)
            yield key >> 3, at, at + length
            at += length
        else:
            sys.exit(f"unsupported protobuf wire type {wire}")


def id_ranges(data):
    """Where every trace and span id of an ExportTraceServiceRequest lies: each span's trace_id (1), span_id (2)
    and parent_span_id (4), and each of its links' trace_id (1) and span_id (2)."""
    ranges = []
    for f1, s1, e1 in fields(data, 0, len(data)):
        if f1 != 1:
            continue
        for f2, s2, e2 in fields(data, s1, e1):
            if f2 != 2:
                continue
            for f3, s3, e3 in fields(data, s2, e2):
                if f3 != 2:
                    continue
                for f4, s4, e4 in fields(data, s3, e3):
                    if f4 in (1, 2, 4):
                        ranges.append((s4, e4))
                    elif f4 == 13:
                        ranges.extend((s5, e5) for f5, s5, e5 in fields(data, s4, e4) if f5 in (1, 2))
    return ranges


requests = []
for path in sorted(Path(fixture).glob("*.pb")):
    data = path.read_bytes()
    requests.append((data, id_ranges(data)))
if not any(ranges for _, ranges in requests):
    sys.exit("the fixture carries no span ids to rewrite")

lock = threading.Lock()
posted = open(work / "posted", "a")


def refuse(message):
    with lock, open(work / "post-errors", "a") as errors:
        errors.write(message + "\n")


def loader(number):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    passes = 0
    while not stop.exists():
        passes += 1
        # One key per loader and pass: a trace whose spans arrive in several requests of the fixture stays one
        # trace, and no two posts of the run carry the same ids.
        key = random.Random(number * 1_000_003 + passes).randbytes(16)
        for data, ranges in requests:
            if stop.exists():
                return
            body = bytearray(data)
            for start, end in ranges:
                for at in range(start, end):
                    body[at] ^= key[at - start]
            try:
                connection.request(
                    "POST",
                    "/otel/default/v1/traces",
                    body=bytes(body),
                    headers={"Content-Type": "application/x-protobuf"},
                )
                response = connection.getresponse()
                response.read()
            except (OSError, http.client.HTTPException) as error:
                refuse(f"loader {number}: request failed: {error!r}")
                return
            if response.status != 200:
                refuse(f"loader {number} got HTTP {response.status}")
                return
            with lock:
                posted.write("x\n")
                posted.flush()
            time.sleep(pace)


threads = [threading.Thread(target=loader, args=(number,)) for number in range(1, loaders + 1)]
for thread in threads:
    thread.start()
for thread in threads:
    thread.join()
LOADERS

wait_healthy() {
  for _ in $(seq 1 90); do
    curl_f "http://127.0.0.1:$PORT/api/v1/health" >/dev/null && return 0
    sleep 1
  done
  echo "[footprint] server did not come up" >&2
  server_log >&2
  exit 1
}

# Settles the server and reads it idle, into IDLE_CHARGED and IDLE_RSS.
settle_idle() {
  sleep "$IDLE_SETTLE_SECS"
  IDLE_CHARGED=0
  IDLE_RSS=0
  local stable=0 prev=0 current reading delta
  for _ in $(seq 1 60); do
    reading="$(sample_or_fail)"
    current="${reading%% *}"
    if [ "$prev" -gt 0 ]; then
      delta=$(( current > prev ? current - prev : prev - current ))
      if [ "$delta" -lt "$IDLE_STABLE_DELTA_BYTES" ]; then stable=$((stable + 1)); else stable=0; fi
    fi
    prev="$current"
    # Three consecutive stable readings, not one: a single small delta happens in the middle of a phase.
    if [ "$stable" -ge 3 ]; then
      IDLE_CHARGED="$current"
      IDLE_RSS="${reading##* }"
      return 0
    fi
    sleep "$SAMPLE_INTERVAL_SECS"
  done
  fail "idle memory never stabilised; last reading $(mb "$prev") MB"
}

# How many spans one pass of the fixture carries, into REQUESTS and SPANS_PER_PASS.
#
# Needed to report the achieved span rate, and it has to be measured rather than assumed: the requests of one
# fixture carry very different span counts. One full pass first, then the store's own count. The load sends the
# same requests under fresh ids, so the count of one pass is the count of every pass.
prime() {
  echo "[footprint] loading one pass of $FIXTURE_NAME to learn its span count"
  REQUESTS=0
  local f status
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
}

# Validate the fraction before passing it to `awk`; arithmetic errors inside a shell condition do not reliably
# trigger `set -e`. Reject punctuation-only and multi-decimal values as well as non-numeric characters.
# `FOOTPRINT_MIN_RATE_FRACTION` is what an operator lowers deliberately, which leaves a record in the command
# rather than in a note nobody reads.
RATE_FRACTION="${FOOTPRINT_MIN_RATE_FRACTION:-0.9}"
case "$RATE_FRACTION" in
  '' | . | *[!0-9.]* | *.*.*) fail "FOOTPRINT_MIN_RATE_FRACTION must be a number, got '$RATE_FRACTION'" ;;
esac

# Steady ingest at a rate, sampled while it runs. Sets MEDIAN_CHARGED, MAX_CHARGED, MEDIAN_RSS, MAX_RSS, ACHIEVED,
# MIN_RATE, POSTED, ELAPSED, SURVIVED (no when the server died under the load) and INGEST_UNMEASURED (why the
# samples do not describe the stated workload, or empty).
#
# Load and sampling run concurrently: a sample taken between requests is not a reading of the ingest state.
# Each loader is paced toward the rate the figure is stated at. An unbounded generator made a fast host run at
# 8 000+ spans/s and then compared that figure with the 5 000 spans/s ceiling; that is a different workload in the
# opposite direction from the low-rate false pass guarded below. The 0.75 factor leaves room for request latency,
# while the mandatory achieved-rate floor still rejects a host that does not reach the stated regime.
steady_ingest() {
  local rate="$1" open="$2"
  local pace stop="$WORK/stop" reading start loader
  pace="$(awk -v loaders="$LOADERS" -v spans="$SPANS_PER_PASS" -v reqs="$REQUESTS" -v target="$rate" \
    'BEGIN { if (target > 0 && reqs > 0) printf "%.6f", 0.75 * loaders * spans / (reqs * target); else print 0 }')"
  rm -f "$stop" "$WORK/post-errors"
  : >"$WORK/posted"
  python3 "$WORK/loaders.py" "$FIXTURE" "$WORK" "$LOADERS" "$PORT" "$LOADER_TIMEOUT_SECS" "$pace" "$stop" &
  loader=$!

  local charged=() rss=()
  SURVIVED=yes
  start="$(date +%s)"
  while [ $(( $(date +%s) - start )) -lt "$INGEST_SECS" ]; do
    reading="$(sample)"
    if [ -z "$reading" ]; then SURVIVED=no; break; fi
    charged+=("${reading%% *}")
    rss+=("${reading##* }")
    sleep "$SAMPLE_INTERVAL_SECS"
  done
  ELAPSED=$(( $(date +%s) - start ))
  touch "$stop"
  # The loader **by pid**, never a bare `wait`.
  #
  # A bare `wait` waits for every background child, and a local server is one of them - so it blocked until the
  # server exited, which only the EXIT trap does, and the trap cannot run while `wait` is blocked. The run hung
  # here forever and never evaluated a single ceiling. A gate that cannot reach its own verdict is worse than no
  # gate: it looks like a slow machine.
  wait "$loader" 2>/dev/null || true

  if [ "${#charged[@]}" -eq 0 ]; then
    # Dead before the first reading is still a verdict under a limit, so it is the caller's to give.
    [ "$SURVIVED" = no ] || fail "no samples taken. Log: $(server_log)"
    charged=(0)
    rss=(0)
  fi
  MEDIAN_CHARGED="$(median "${charged[@]}")"
  MAX_CHARGED="$(largest "${charged[@]}")"
  MEDIAN_RSS="$(median "${rss[@]}")"
  MAX_RSS="$(largest "${rss[@]}")"
  POSTED="$(wc -l <"$WORK/posted" | tr -d ' ')"
  # Average spans per request across the fixture: the requests are not uniform, so this is an average by
  # construction and is reported as an achieved rate rather than asserted as one.
  ACHIEVED="$(awk -v posted="$POSTED" -v spans="$SPANS_PER_PASS" -v reqs="$REQUESTS" -v secs="$ELAPSED" \
    'BEGIN { if (secs > 0 && reqs > 0) printf "%.0f", posted * (spans / reqs) / secs; else print 0 }')"
  # Below the rate the figure is stated at, it describes a different workload - so this is a failed
  # *measurement*, not a passed gate. A system at 300 MB under 2 500 spans/s can be at 500 MB under 5 000, so the
  # floor is just below the target rather than at half of it.
  MIN_RATE="$(awk -v target="$rate" -v frac="$RATE_FRACTION" 'BEGIN { printf "%.0f", target * frac }')"
  case "$MIN_RATE" in
    '' | *[!0-9]*) fail "could not compute a rate floor from target=$rate fraction=$RATE_FRACTION" ;;
  esac
  # And a floor of zero is no floor, however it was arrived at. Refused rather than reported, because a gate that
  # admits everything while claiming a threshold is worse than an absent gate.
  [ "$MIN_RATE" -gt 0 ] || fail "the computed rate floor is 0, which enforces nothing (target=$rate fraction=$RATE_FRACTION)"

  # A 503 here is admission, `BufferFull` or a rate limit - the server protecting itself, a legitimate answer,
  # and not load. Reported as a failure of the *measurement*, because the figure taken while the server was
  # refusing describes a server doing less work than the ceiling claims. Recorded rather than exited on, so
  # every other figure is still reported: a run that stops at the first failure hides what else it would have
  # found.
  INGEST_UNMEASURED=""
  if [ -s "$WORK/post-errors" ]; then
    INGEST_UNMEASURED="the server stopped accepting exports, so the samples do not describe steady ingest: $(sort -u "$WORK/post-errors" | head -3 | tr '\n' ' '); $open"
  elif [ "$ACHIEVED" -lt "$MIN_RATE" ]; then
    INGEST_UNMEASURED="achieved ~$ACHIEVED spans/s, below the $MIN_RATE floor for a figure stated at $rate spans/s, so it describes a lighter workload than the one stated; $open"
  fi
}

report_ingest() {
  local label="$1" rate="$2"
  echo "[footprint] $label: charged median $(mb "$MEDIAN_CHARGED") MB, max $(mb "$MAX_CHARGED") MB (ungated); RSS median $(mb "$MEDIAN_RSS") MB, max $(mb "$MAX_RSS") MB (ungated); over ${ELAPSED}s"
  echo "[footprint] $label: achieved ~$ACHIEVED spans/s from $POSTED requests across $LOADERS loaders (stated at $rate spans/s, floor $MIN_RATE)"
}

THROUGHPUT_OPEN="the ${TARGET_SPANS_PER_SECOND} spans/s rate is not reached yet (the write-path slices 5-11), so this gate stays failing until it is"
TARGET_OPEN="the ${TARGET_HOST_SPANS_PER_SECOND} spans/s per core target is not reached yet (per-span CPU and the write-path slices), so this run stays failing until it is"

echo "[footprint] measuring the memory the OS charges the server - phys_footprint on macOS, its cgroup's memory.current on Linux - which is what a memory limit is enforced against. RSS is reported beside it, ungated: on macOS it also counts freed pages the kernel takes back at will, and on Linux it leaves out the page cache and kernel memory the cgroup is charged for"

# --- local: this host's release binary ------------------------------------------------------------------------
if [ "$MODE" = "local" ]; then
  # Release, always: a debug build's footprint describes the debug build, and the ceilings are stated on what
  # ships.
  echo "[footprint] building"
  (cd "$ROOT" && cargo build --locked --release -q -p sideseat-server)

  # On Linux the figure is a cgroup's, so the server needs one of its own: `memory.current` of a shared cgroup
  # charges every other process in it to the server. A transient systemd scope gives it one where a user manager
  # runs; elsewhere the server's cgroup must already hold it alone, and the run says so rather than measuring
  # its neighbours.
  SCOPE=()
  if [ "$OS" = "Linux" ] && command -v systemd-run >/dev/null 2>&1 &&
    systemd-run --user --scope --quiet true >/dev/null 2>&1; then
    SCOPE=(systemd-run --user --scope --quiet --collect --unit "sideseat-footprint-$$")
  fi

  # `env -i` with an explicit environment, for the same reason `bench-http-latency.sh` does it: an inherited
  # `SIDESEAT_*` variable or a `~/.sideseat/sideseat.json` would silently change the backend, the retention or
  # the caches, and a footprint figure would then come from a configuration nobody recorded. `exec`, so `$!` is
  # the server's own pid rather than a shell that exits immediately - without it `stop_server` kills nothing and
  # the samples read a dead process. `systemd-run --scope` execs too, after moving itself into the scope.
  echo "[footprint] starting server on :$PORT"
  (cd "$WORK" && exec ${SCOPE[@]+"${SCOPE[@]}"} env -i \
    PATH="$PATH" HOME="$WORK" \
    SIDESEAT_DATA_DIR="$WORK" SIDESEAT_SECRETS_BACKEND=file \
    SIDESEAT_PORT="$PORT" SIDESEAT_UI_PORT="$((PORT + 1))" \
    SIDESEAT_OTEL_GRPC_PORT="$((PORT + 2))" \
    "$CARGO_TARGET_DIR/release/sideseat" --no-auth >"$WORK/server.log" 2>&1) &
  SERVER_PID=$!
  wait_healthy
  if [ "$OS" = "Linux" ]; then
    CGROUP_DIR="/sys/fs/cgroup$(awk -F: '$1 == "0" { print $3 }' "/proc/$SERVER_PID/cgroup")"
    [ -r "$CGROUP_DIR/memory.current" ] ||
      fail "the server's cgroup $CGROUP_DIR has no memory controller, so nothing charges it; run under systemd-run --user, or use \`$0 container\`"
    [ "$(tr '\n' ' ' <"$CGROUP_DIR/cgroup.procs" | sed 's/ $//')" = "$SERVER_PID" ] ||
      fail "the server shares cgroup $CGROUP_DIR with other processes, whose memory.current it would be charged; run where systemd-run --user works, or use \`$0 container\`"
    echo "[footprint] cgroup $CGROUP_DIR"
  elif [ "$OS" != "Darwin" ]; then
    fail "no charged-memory reading on $OS; use \`$0 container\`"
  fi

  echo "[footprint] gate 1: idle memory (ceiling $(mb $IDLE_MEMORY_CEILING_BYTES) MB)"
  settle_idle
  echo "[footprint] idle: charged $(mb "$IDLE_CHARGED") MB; RSS $(mb "$IDLE_RSS") MB (ungated)"

  prime
  echo "[footprint] gate 2: memory under steady ingest (ceiling $(mb $INGEST_MEMORY_CEILING_BYTES) MB)"
  steady_ingest "$TARGET_SPANS_PER_SECOND" "$THROUGHPUT_OPEN"
  [ "$SURVIVED" = yes ] || fail "the server died under the load. Log: $(server_log)"
  report_ingest "steady ingest" "$TARGET_SPANS_PER_SECOND"

  FAILURES=0
  if [ "$IDLE_CHARGED" -gt "$IDLE_MEMORY_CEILING_BYTES" ]; then
    echo "[footprint] FAIL: idle memory $(mb "$IDLE_CHARGED") MB exceeds $(mb $IDLE_MEMORY_CEILING_BYTES) MB" >&2
    FAILURES=$((FAILURES + 1))
  fi
  if [ -n "$INGEST_UNMEASURED" ]; then
    echo "[footprint] FAIL: $INGEST_UNMEASURED" >&2
    FAILURES=$((FAILURES + 1))
  fi
  # Judged even when the rate fell short: a lighter workload that already exceeds the ceiling is evidence against
  # it, where one under the ceiling is evidence of nothing.
  if [ "$MEDIAN_CHARGED" -gt "$INGEST_MEMORY_CEILING_BYTES" ]; then
    echo "[footprint] FAIL: steady ingest median $(mb "$MEDIAN_CHARGED") MB exceeds $(mb $INGEST_MEMORY_CEILING_BYTES) MB" >&2
    FAILURES=$((FAILURES + 1))
  fi
  [ "$FAILURES" -eq 0 ] || exit 1
  echo "[footprint] both memory ceilings met"
  exit 0
fi

# --- container: the Linux image under enforced limits -----------------------------------------------------------
#
# The image is built from this checkout every run, unless `FOOTPRINT_IMAGE` names one: a limit enforced on an
# image of other code is a statement about that code. Docker's build cache keeps a rebuild to what changed.
command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1 ||
  fail "container mode needs a running Docker (on macOS: colima start)"
IMAGE="${FOOTPRINT_IMAGE:-}"
if [ -z "$IMAGE" ]; then
  IMAGE="sideseat-bench:footprint"
  echo "[footprint] building $IMAGE from this checkout"
  docker build -t "$IMAGE" -f "$ROOT/scripts/deploy/bench/Dockerfile" "$ROOT" >"$WORK/build.log" 2>&1 ||
    { tail -30 "$WORK/build.log" >&2; fail "the image did not build"; }
fi
HOST_CORES="$(docker info -f '{{.NCPU}}')"
echo "[footprint] image $IMAGE on $(docker info -f '{{.OperatingSystem}} {{.Architecture}}'), $HOST_CORES cores"

# One run of the image under a limit: the idle figure, then steady ingest at a rate. Never refused memory is not
# the claim here; surviving the limit at the rate is.
FAILURES=0
limited_run() {
  local label="$1" limit="$2" cores="$3" rate="$4" open="$5"
  CONTAINER="sideseat-footprint-$$-$label"
  echo "[footprint] $label: limit $(mb "$limit") MB, $cores cores, $rate spans/s"
  # `--memory-swap` equal to `--memory`: no swap, so the limit is on memory rather than on memory plus disk.
  # The data directory is the container's own filesystem, never a tmpfs, whose pages the cgroup would charge as
  # memory the server did not allocate.
  docker run -d --name "$CONTAINER" -p "$PORT:5388" \
    --memory "${limit}b" --memory-swap "${limit}b" \
    --cpus "$cores" --cpuset-cpus "0-$((cores - 1))" \
    "$IMAGE" --no-auth >/dev/null
  wait_healthy
  settle_idle
  echo "[footprint] $label idle: charged $(mb "$IDLE_CHARGED") MB; RSS $(mb "$IDLE_RSS") MB (ungated)"
  prime
  steady_ingest "$rate" "$open"
  local state events stat
  state="$(docker inspect -f '{{.State.Running}} {{.State.OOMKilled}} {{.State.ExitCode}}' "$CONTAINER")"
  events="$(docker exec "$CONTAINER" cat /sys/fs/cgroup/memory.events 2>/dev/null | tr '\n' ' ' || true)"
  # What the charged figure is made of at the end - anonymous memory, the page cache of the store's files, the
  # kernel's own - since only the first is the program's to bound and the second is reclaimed under the limit.
  stat="$(docker exec "$CONTAINER" cat /sys/fs/cgroup/memory.stat 2>/dev/null |
    awk '$1 == "anon" || $1 == "file" || $1 == "kernel" { printf "%s %.1f MB  ", $1, $2 / 1048576 }' || true)"
  report_ingest "$label" "$rate"
  [ -z "$stat" ] || echo "[footprint] $label memory.stat at the end: $stat"
  [ -z "$events" ] || echo "[footprint] $label memory.events: $events"
  if [ "$SURVIVED" != yes ] || [ "${state%% *}" != "true" ]; then
    echo "[footprint] FAIL: $label: the server did not survive the limit (running, OOM-killed, exit code: $state). Log: $(server_log)" >&2
    FAILURES=$((FAILURES + 1))
  elif printf '%s' "$events" | grep -Eq 'oom_kill [1-9]'; then
    echo "[footprint] FAIL: $label: the kernel killed a process of the server's under the limit ($events)" >&2
    FAILURES=$((FAILURES + 1))
  fi
  if [ -n "$INGEST_UNMEASURED" ]; then
    echo "[footprint] FAIL: $label: $INGEST_UNMEASURED" >&2
    FAILURES=$((FAILURES + 1))
  fi
  stop_server
}

[ "$HOST_CORES" -ge "$TARGET_HOST_CORES" ] || fail "the Docker host has $HOST_CORES cores, fewer than the target host's $TARGET_HOST_CORES"
# The ingest ceiling as a hard limit, with every core the Docker host has: more cores is more worker threads,
# allocator arenas and DuckDB threads, so this is the heavier side of the claim.
limited_run ceiling "$INGEST_MEMORY_CEILING_BYTES" "$HOST_CORES" "$TARGET_SPANS_PER_SECOND" "$THROUGHPUT_OPEN"
# The target host: one core and 2 GB for the server and its embedded backend, at the target rate.
limited_run target-host "$TARGET_HOST_MEMORY_BYTES" "$TARGET_HOST_CORES" "$TARGET_HOST_SPANS_PER_SECOND" "$TARGET_OPEN"
[ "$FAILURES" -eq 0 ] || exit 1
echo "[footprint] the server held both enforced limits at their rates"
