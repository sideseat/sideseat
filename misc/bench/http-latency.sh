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
# A gap between sequential samples, so the numbers are service time rather than queueing delay.
#
# Without it, posting a 754 KB payload back to back measures saturation: each request waits for the
# previous write, so the tail is queue depth and the whole distribution above the median moves run to run.
# Measured across four runs it did exactly that - p95 43, 53, 56, 135 ms - which makes any ceiling on it a
# coin flip. An OTLP exporter batches on a schedule (5 s by default) and never behaves this way. The
# 8-concurrent read is the deliberate concurrency measurement, and it keeps no gap.
GAP_MS="${BENCH_GAP_MS:-25}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$(mktemp -d)"
PG_NAME=sideseat-bench-pg
CH_NAME=sideseat-bench-ch
MINIO_NAME=sideseat-bench-minio
SERVER_PID=""

cleanup() {
  # This exact process, never `pkill -f sideseat`: running the benchmark next to a developer's own no-auth
  # server would otherwise kill theirs too.
  if [ -n "$SERVER_PID" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [ "$MODE" = "distributed" ]; then
    docker rm -f "$PG_NAME" "$CH_NAME" "$MINIO_NAME" >/dev/null 2>&1 || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "[bench] building release"
(cd "$ROOT" && cargo build --release -q -p sideseat-server)

if [ "$MODE" = "distributed" ]; then
  command -v docker >/dev/null || { echo "[bench] docker is required for distributed mode"; exit 1; }
  docker rm -f "$PG_NAME" "$CH_NAME" "$MINIO_NAME" >/dev/null 2>&1 || true
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
  # MinIO, because the shared-store rule refuses PostgreSQL with filesystem storage - a row every
  # instance can see naming content only one machine holds. Enforced at startup, so the distributed
  # benchmark has to be a *coherent* distributed deployment, which makes its numbers the right ones:
  # object storage is what a scaled-out instance actually pays for a file.
  docker run -d --name "$MINIO_NAME" -p 9010:9000 \
    -e MINIO_ROOT_USER=sideseat -e MINIO_ROOT_PASSWORD=sideseat12345 \
    minio/minio:RELEASE.2025-04-22T22-12-26Z server /data >/dev/null
  for _ in $(seq 1 60); do
    curl -sf http://127.0.0.1:9010/minio/health/live >/dev/null && break
    sleep 1
  done
  docker run --rm --network host --entrypoint sh minio/mc:RELEASE.2025-04-16T18-13-26Z -c \
    'mc alias set bench http://127.0.0.1:9010 sideseat sideseat12345 >/dev/null &&
     mc mb --ignore-existing bench/sideseat-bench >/dev/null' >/dev/null

  cat > "$WORK/sideseat.json" <<'JSON'
{
  "files": {
    "enabled": true,
    "storage": "s3",
    "s3": {
      "bucket": "sideseat-bench",
      "prefix": "files",
      "region": "us-east-1",
      "endpoint": "http://127.0.0.1:9010"
    }
  },
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
# MinIO credentials, from variables rather than the config file, because the AWS SDK reads them from the
# environment and a throwaway password in a committed file is the pattern the secret scanner exists to catch.
if [ "$MODE" = "distributed" ]; then
  export AWS_ACCESS_KEY_ID=sideseat AWS_SECRET_ACCESS_KEY=sideseat12345 AWS_REGION=us-east-1
fi
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
pace() {
  [ "$GAP_MS" -gt 0 ] && sleep "$(python3 -c "print($GAP_MS/1000)")"
  return 0
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

# The *first* read of this session, before anything is warmed. A replica starts with an empty
# reconstruction cache and a deploy replaces every replica, so this is not a curiosity - it is what the
# first reader of any session pays, on every instance, and a warm-only SLO says nothing about it.
: > "$WORK/read-cold.txt"
timed_get "$MSGS" "$WORK/read-cold.txt"
COLD_MS="$(python3 -c "print(f'{float(open('$WORK/read-cold.txt').read().strip())*1000:.1f}')")"
echo "[bench] cold read (empty reconstruction cache): $COLD_MS ms"

echo "[bench] warming up ($WARMUP requests each, discarded)"
: > "$WORK/warm.txt"
for _ in $(seq 1 "$WARMUP"); do timed_post "$SMALL" "$WORK/warm.txt"; timed_get "$MSGS" "$WORK/warm.txt"; done

echo "[bench] measuring ($SAMPLES samples each)"
: > "$WORK/ingest-small.txt"; : > "$WORK/ingest-large.txt"
: > "$WORK/read.txt"; : > "$WORK/list.txt"
for _ in $(seq 1 "$SAMPLES"); do timed_post "$SMALL" "$WORK/ingest-small.txt"; pace; done
for _ in $(seq 1 $((SAMPLES / 2))); do timed_post "$LARGE" "$WORK/ingest-large.txt"; pace; done
for _ in $(seq 1 "$SAMPLES"); do timed_get "$MSGS" "$WORK/read.txt"; pace; done
for _ in $(seq 1 "$SAMPLES"); do
  timed_get "http://127.0.0.1:$PORT/api/v1/project/default/otel/traces?limit=50" "$WORK/list.txt"
  pace
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
  SPANS="$SPANS" MIN_P99_SAMPLES="$MIN_P99_SAMPLES" COLD_MS="$COLD_MS" python3 - "$WORK" <<'PY'
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
measured = {}
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
    # p95, which is the statistic the SLO gates on - see below.
    measured[name] = pct(.95)
print(f"\nCold read, empty reconstruction cache: {os.environ['COLD_MS']} ms\n")

# The SLO, enforced rather than printed. A benchmark whose numbers nobody compares against a target is a
# report, not a check: the tables in CLAUDE.md could drift arbitrarily far from the documented promise and
# every run would still exit 0.
#
# Gated on **p95**, and the p99 column is reported without being gated. That is not a softer target, it is
# the only one that means anything at these sample counts: with 200 samples the p99 is the second-worst
# request, so it is set by whatever else the host was doing, and a ceiling on it passes or fails by luck.
# Measured, back to back on one host: the 754 KB export's p99 ranged from 125 ms to 667 ms across runs
# while its p50 stayed at 20 ms and its p95 at 50-80 ms. Isolating it (same payload, `files.enabled` off)
# showed the same tail, so it is DuckDB's own write amortisation rather than file extraction - inherent to
# an embedded store absorbing 750 KB writes with no gap between them, which is not how an exporter behaves.
SLO_MS = {
    "embedded": {
        "ingest-small.txt": 10,
        "ingest-large.txt": 100,
        "read.txt": 40,
        "read-conc.txt": 150,
        "list.txt": 40,
    },
    "distributed": {
        "ingest-small.txt": 80,
        "ingest-large.txt": 120,
        "read.txt": 600,
        "read-conc.txt": 1500,
        "list.txt": 700,
    },
}
breaches = [
    f"{name}: p95 {value:.1f} ms against a {SLO_MS[mode][name]} ms ceiling"
    for name, value in measured.items()
    if name in SLO_MS.get(mode, {}) and value > SLO_MS[mode][name]
]
if breaches:
    print(f"[bench] SLO breached for {mode}:")
    for breach in breaches:
        print(f"  - {breach}")
    sys.exit(1)
print(f"[bench] {mode}: every operation is within its documented p95 ceiling")
PY
