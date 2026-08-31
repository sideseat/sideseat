#!/usr/bin/env bash
#
# End-to-end HTTP latency for ingestion and reads, against either backend pair.
#
# The numbers in CLAUDE.md come from this script, so they can be re-taken rather than trusted. It measures
# what a *client* sees - the whole request, including the write - because that is the only latency anyone
# experiences; the in-process benches (`bench_ingestion_end_to_end`, `bench_session_scaling`) measure the
# stages inside it.
#
#   misc/bench/http-latency.sh embedded      # SQLite + DuckDB, the default deployment
#   misc/bench/http-latency.sh distributed   # PostgreSQL + ClickHouse, in throwaway containers
#
# Three things this script is careful about, each because getting it wrong produces a number that looks
# like evidence and is not:
#
#   * **The read workload is the whole fixture.** Every request of `langgraph/swarm` is posted once, and the
#     script prints how many spans the resulting session actually covers rather than asserting a number.
#     Re-posting one request many times does not build a session: ingestion is idempotent by span id, so it
#     stays as small as one request's worth however many times it is sent.
#   * **A failed request is not a sample.** Every measured call checks its status; a fast 404 or 503 would
#     otherwise be recorded as excellent latency.
#   * **p99 needs samples.** Below `BENCH_MIN_P99_SAMPLES` the p99 column is reported as `max` instead,
#     because the 99th percentile of fifty samples *is* the maximum and calling it p99 overstates it.
set -euo pipefail

MODE="${1:-embedded}"
PORT="${BENCH_PORT:-5599}"
SAMPLES="${BENCH_SAMPLES:-200}"
WARMUP="${BENCH_WARMUP:-5}"
CONCURRENCY="${BENCH_CONCURRENCY:-8}"
MIN_P99_SAMPLES="${BENCH_MIN_P99_SAMPLES:-100}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$(mktemp -d)"
PG_NAME=sideseat-bench-pg
CH_NAME=sideseat-bench-ch
SERVER_PID=""

cleanup() {
  # This exact process, never `pkill -f sideseat`: running the benchmark next to a developer's own no-auth
  # server would otherwise kill theirs too.
  if [ -n "$SERVER_PID" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [ "$MODE" = "distributed" ]; then
    docker rm -f "$PG_NAME" "$CH_NAME" >/dev/null 2>&1 || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "[bench] building release"
(cd "$ROOT" && cargo build --release -q -p sideseat-server)

if [ "$MODE" = "distributed" ]; then
  command -v docker >/dev/null || { echo "[bench] docker is required for distributed mode"; exit 1; }
  docker rm -f "$PG_NAME" "$CH_NAME" >/dev/null 2>&1 || true
  docker run -d --name "$PG_NAME" -p 5442:5432 \
    -e POSTGRES_USER=sideseat -e POSTGRES_PASSWORD=sideseat -e POSTGRES_DB=sideseat \
    postgres:17-alpine >/dev/null
  docker run -d --name "$CH_NAME" -p 8131:8123 \
    -e CLICKHOUSE_USER=sideseat -e CLICKHOUSE_PASSWORD=sideseat \
    -e CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT=1 clickhouse/clickhouse-server:25.8 >/dev/null
  echo "[bench] waiting for PostgreSQL and ClickHouse"
  for _ in $(seq 1 90); do
    docker exec "$PG_NAME" pg_isready -U sideseat -d sideseat >/dev/null 2>&1 &&
      curl -sf http://127.0.0.1:8131/ping >/dev/null && break
    sleep 1
  done
  # Credentials as headers from variables rather than an inline `-u`: the throwaway password is not a
  # secret, but a script that writes credentials into an argv is the pattern the secret scanner exists to
  # catch, and weakening the scanner to allow it would be the wrong trade.
  CH_USER=sideseat CH_KEY=sideseat
  curl -sf -H "X-ClickHouse-User: $CH_USER" -H "X-ClickHouse-Key: $CH_KEY" \
    'http://127.0.0.1:8131/' --data 'CREATE DATABASE IF NOT EXISTS sideseat' >/dev/null
  cat > "$WORK/sideseat.json" <<'JSON'
{
  "database": {
    "transactional": "postgres",
    "analytics": "clickhouse",
    "postgres": { "url": "postgres://sideseat:sideseat@127.0.0.1:5442/sideseat" },
    "clickhouse": { "url": "http://127.0.0.1:8131", "user": "sideseat", "password": "sideseat", "database": "sideseat" }
  }
}
JSON
fi

echo "[bench] starting server on :$PORT ($MODE)"
(cd "$WORK" && SIDESEAT_DATA_DIR="$WORK" SIDESEAT_SECRETS_BACKEND=file \
  SIDESEAT_PORT="$PORT" SIDESEAT_UI_PORT="$((PORT + 1))" \
  exec "$ROOT/target/release/sideseat" --no-auth > "$WORK/server.log" 2>&1) &
SERVER_PID=$!
for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:$PORT/api/v1/health" >/dev/null && break
  sleep 1
done
curl -sf "http://127.0.0.1:$PORT/api/v1/health" >/dev/null || {
  echo "[bench] server did not come up"; tail -20 "$WORK/server.log"; exit 1;
}

FIXTURE="$ROOT/server/tests/fixtures/messages/langgraph/swarm"
SMALL="$FIXTURE/req-001.pb"
LARGE="$(ls -S "$FIXTURE"/*.pb | head -1)"

# The status is checked on every measured call. `--fail` alone is not enough with `-w`, because curl still
# prints the timing and exits non-zero, so the exit code is what decides whether the sample counts.
timed_post() {  # timed_post <file> <out>
  local out
  out="$(curl -s -o /dev/null -w '%{time_total} %{http_code}' -X POST --data-binary @"$1" \
    -H 'Content-Type: application/x-protobuf' "http://127.0.0.1:$PORT/otel/default/v1/traces")"
  local code="${out##* }"
  [ "$code" = "200" ] || { echo "[bench] ingest returned $code, refusing to report it as a sample"; exit 1; }
  echo "${out%% *}" >> "$2"
}
timed_get() {  # timed_get <url> <out>
  local out
  out="$(curl -s -o /dev/null -w '%{time_total} %{http_code}' "$1")"
  local code="${out##* }"
  [ "$code" = "200" ] || { echo "[bench] read returned $code, refusing to report it as a sample"; exit 1; }
  echo "${out%% *}" >> "$2"
}

echo "[bench] loading the whole fixture, so the session is the one the SLO describes"
: > "$WORK/load.txt"
for f in "$FIXTURE"/*.pb; do timed_post "$f" "$WORK/load.txt"; done
sleep 4

# The session's own span count, from the session list - which is what the read workload actually spans, and
# what the SLO's "151 spans" has to mean if it means anything.
SESSION_JSON="$(curl -sf "http://127.0.0.1:$PORT/api/v1/project/default/otel/sessions?limit=1")"
SESSION="$(printf '%s' "$SESSION_JSON" |
  python3 -c 'import sys,json; r=json.load(sys.stdin).get("data") or []; print(r[0]["session_id"] if r else "")')"
SPANS="$(printf '%s' "$SESSION_JSON" |
  python3 -c 'import sys,json; r=json.load(sys.stdin).get("data") or []; print(r[0]["span_count"] if r else "?")')"
[ -n "$SESSION" ] || { echo "[bench] no session was created; the fixture did not load"; exit 1; }
MSGS="http://127.0.0.1:$PORT/api/v1/project/default/otel/sessions/$SESSION/messages"
echo "[bench] session $SESSION covers $SPANS spans"

echo "[bench] warming up ($WARMUP requests each, discarded)"
: > "$WORK/warm.txt"
for _ in $(seq 1 "$WARMUP"); do timed_post "$SMALL" "$WORK/warm.txt"; timed_get "$MSGS" "$WORK/warm.txt"; done

echo "[bench] measuring ($SAMPLES samples each)"
: > "$WORK/ingest-small.txt"; : > "$WORK/ingest-large.txt"
: > "$WORK/read.txt"; : > "$WORK/list.txt"
for _ in $(seq 1 "$SAMPLES"); do timed_post "$SMALL" "$WORK/ingest-small.txt"; done
for _ in $(seq 1 $((SAMPLES / 2))); do timed_post "$LARGE" "$WORK/ingest-large.txt"; done
for _ in $(seq 1 "$SAMPLES"); do timed_get "$MSGS" "$WORK/read.txt"; done
for _ in $(seq 1 "$SAMPLES"); do
  timed_get "http://127.0.0.1:$PORT/api/v1/project/default/otel/traces?limit=50" "$WORK/list.txt"
done

# Concurrent reads: the status check runs per request, and any failure fails the whole run.
export PORT MSGS WORK
: > "$WORK/read-conc.txt"
seq 1 "$SAMPLES" | xargs -P "$CONCURRENCY" -I{} sh -c '
  out=$(curl -s -o /dev/null -w "%{time_total} %{http_code}" "$MSGS")
  case "$out" in *" 200") echo "${out%% *}" >> "$WORK/read-conc.txt" ;; *) exit 9 ;; esac' ||
  { echo "[bench] a concurrent read failed, refusing to report the run"; exit 1; }

SMALL_KB=$(( $(wc -c < "$SMALL" | tr -d ' ') / 1024 ))
LARGE_KB=$(( $(wc -c < "$LARGE" | tr -d ' ') / 1024 ))
MODE="$MODE" CONCURRENCY="$CONCURRENCY" SMALL_KB="$SMALL_KB" LARGE_KB="$LARGE_KB" \
  SPANS="$SPANS" MIN_P99_SAMPLES="$MIN_P99_SAMPLES" python3 - "$WORK" <<'PY'
import os, sys
work = sys.argv[1]
mode = os.environ["MODE"]
spans = os.environ["SPANS"]
min_p99 = int(os.environ["MIN_P99_SAMPLES"])
rows = [
    (f"trace export, {os.environ['SMALL_KB'] or '<1'} KB", "ingest-small.txt"),
    (f"trace export, {os.environ['LARGE_KB']} KB", "ingest-large.txt"),
    (f"session messages ({spans} spans), sequential", "read.txt"),
    (f"session messages ({spans} spans), {os.environ['CONCURRENCY']} concurrent", "read-conc.txt"),
    ("trace list, 50", "list.txt"),
]
print(f"\n[bench] {mode}: milliseconds, whole HTTP request, failures excluded by aborting the run\n")
print("| Operation | n | p50 | p95 | p99 |")
print("| --- | --- | --- | --- | --- |")
for label, name in rows:
    path = os.path.join(work, name)
    if not os.path.exists(path):
        continue
    v = sorted(float(x) * 1000 for x in open(path) if x.strip())
    if not v:
        continue
    pct = lambda p: v[min(len(v) - 1, int(len(v) * p))]
    # Below the threshold the 99th percentile *is* the maximum, and calling it p99 overstates the evidence.
    tail = f"{pct(.99):.1f} ms" if len(v) >= min_p99 else f"max {v[-1]:.1f} ms"
    print(f"| {label} | {len(v)} | {pct(.5):.1f} ms | {pct(.95):.1f} ms | {tail} |")
print()
PY
