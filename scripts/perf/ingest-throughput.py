#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["opentelemetry-proto==1.45.0"]
# ///
"""Sustained trace-ingest throughput at rising offered rates, from corpus-derived load.

    uv run --locked --script scripts/perf/ingest-throughput.py local [--cores 1,2,4] [--min-sustained N]
    uv run --locked --script scripts/perf/ingest-throughput.py container --cores 1,2,4,8 --memory 2g

`local` runs the host's release binary; on Linux it is pinned to the first N cores with `taskset`, and on macOS,
which cannot pin, the run says it is unpinned. `container` runs the aarch64-linux image `scripts/deploy/bench/`
builds, with `docker run --cpus N --cpuset-cpus 0-(N-1) --memory 2g --memory-swap 2g`, so the figure describes a
hard 2 GB box with a pinned core count - the shape of an Ampere A1 instance.

The load is the trace corpus, every export, sent as new telemetry every time. Each pass through a worker's share
of the corpus rewrites every trace and span id with a key unique to that step and pass - parents and links with
the same mapping, so trees stay intact, and every worker uses the same key in the same pass, so a trace whose
spans arrive in several exports stays one trace. Nothing is ever re-sent: a cycled pool would measure the
idempotent redelivery path instead of ingestion.

Each step offers a fixed rate for `--step-secs` seconds (open loop: requests are sent on schedule, not when the
previous one returns, so a slow server shows as latency and a shortfall rather than as a lower offered rate).
A step passes when, at once:

- the achieved rate is at least 95% of the scheduled rate, with no failed request;
- no span was rejected - every response's OTLP `partial_success` is read, not just its status;
- what the server stored is what was accepted: after the step, the distinct spans stored in every project equal
  the distinct span identities the accepted requests carried.

The ramp stops at the first failing step and reports the last passing one as the sustained rate. A step whose
generator fell behind its own schedule is flagged, since then the figure measures the load generator. The run
exits non-zero when stored spans disagree with accepted ones, or when `--min-sustained` is not reached.

Reported per step: offered, scheduled and achieved spans/s, p50/p99 request latency, peak RSS (or container
memory), the generator's lag, and bytes on disk at the end. `--json PATH` records the run.
"""

from __future__ import annotations

import argparse
import collections
import http.client
import json
import multiprocessing
import os
import random
import signal
import subprocess
import sys
import shutil
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / "server/tests/fixtures/messages"
IMAGE = os.environ.get("BENCH_IMAGE", "sideseat-bench:latest")
PORT = int(os.environ.get("BENCH_THROUGHPUT_PORT", "5631"))


def log(message: str) -> None:
    print(f"[throughput] {message}", flush=True)


# --- load ----------------------------------------------------------------------------------------------


def load_requests(include_media: bool) -> list:
    from google.protobuf import json_format
    from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
        ExportTraceServiceRequest,
    )

    requests = []
    for path in sorted(CORPUS.rglob("req-*")):
        if path.suffix not in (".pb", ".json"):
            continue
        message = ExportTraceServiceRequest()
        if path.suffix == ".json":
            json_format.Parse(path.read_bytes(), message)
        else:
            message.ParseFromString(path.read_bytes())
        if not include_media and message.ByteSize() > 512 * 1024:
            continue
        requests.append(message)
    return requests


def pass_key(step: int, number: int) -> bytes:
    """The id key for one pass of one step: unique to both, the same for every worker."""
    return random.Random(step * 1_000_003 + number).randbytes(16)


def rewrite(message, key: bytes):
    """A copy of the export with every trace and span id XORed by `key`: new ids, same tree."""
    out = type(message)()
    out.CopyFrom(message)

    def x(value: bytes) -> bytes:
        return bytes(a ^ b for a, b in zip(value, key)) if value else value

    for rs in out.resource_spans:
        for ss in rs.scope_spans:
            for span in ss.spans:
                span.trace_id = x(span.trace_id)
                span.span_id = x(span.span_id)
                span.parent_span_id = x(span.parent_span_id)
                for link in span.links:
                    link.trace_id = x(link.trace_id)
                    link.span_id = x(link.span_id)
    return out


def identities(message) -> frozenset[tuple[bytes, bytes]]:
    """The distinct (trace id, span id) identities an export carries."""
    return frozenset(
        (span.trace_id, span.span_id)
        for rs in message.resource_spans
        for ss in rs.scope_spans
        for span in ss.spans
    )


def build_pool(
    requests: list,
) -> list[tuple[bytes, int, frozenset[tuple[bytes, bytes]]]]:
    """One template per export: its bytes, its span count and its identities, shuffled once."""
    pool = []
    for message in requests:
        spans = sum(
            len(ss.spans) for rs in message.resource_spans for ss in rs.scope_spans
        )
        pool.append((message.SerializeToString(), spans, identities(message)))
    random.Random(7).shuffle(pool)
    return pool


def keyed(
    ids: frozenset[tuple[bytes, bytes]], key: bytes
) -> frozenset[tuple[bytes, bytes]]:
    """The identities an export carries once its ids are XORed by `key` - the transform `rewrite` applies."""

    def x(value: bytes) -> bytes:
        return bytes(a ^ b for a, b in zip(value, key)) if value else value

    return frozenset((x(trace), x(span)) for trace, span in ids)


# --- workers -------------------------------------------------------------------------------------------


def worker(
    index, workers, pool_path, projects, rate, seconds, step, ready, go, start, results
):
    """Send this worker's share of the offered rate on a fixed schedule and record each request.

    The slice is loaded *before* the clock starts: unpickling tens of megabytes takes seconds, and a schedule
    that started first made every worker begin late and then burst to catch up, which measured the generator.
    Each pass through the slice rewrites the ids with that pass's key, so no request is ever sent twice.
    """
    import pickle

    from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
        ExportTraceServiceRequest,
        ExportTraceServiceResponse,
    )

    mine, spans_per_request = pickle.loads(Path(f"{pool_path}.{index}").read_bytes())
    connection = http.client.HTTPConnection("127.0.0.1", PORT, timeout=60)
    interval = workers * spans_per_request / rate
    records = []
    cursor = 0
    ready.put(index)
    go.wait()
    start_at = start.value
    deadline = start_at + seconds
    next_send = start_at + index * interval / workers
    previous_done = start_at
    while True:
        now = time.monotonic()
        if next_send >= deadline:
            break
        if next_send > now:
            time.sleep(next_send - now)
        # Lag the generator caused: time past the moment it could have sent - the schedule, or the previous
        # response, since one worker holds one request in flight. Waiting on that response is the server's.
        ready_at = max(next_send, previous_done)
        lag = max(0.0, time.monotonic() - ready_at)
        waited = max(0.0, previous_done - next_send)
        slot, number = cursor % len(mine), cursor // len(mine)
        template, spans = mine[slot][1], mine[slot][2]
        message = ExportTraceServiceRequest.FromString(template)
        body = rewrite(message, pass_key(step, number)).SerializeToString()
        project = projects[(cursor + index) % len(projects)]
        cursor += 1
        sent = time.monotonic()
        rejected = 0
        try:
            connection.request(
                "POST",
                f"/otel/{project}/v1/traces",
                body,
                {"Content-Type": "application/x-protobuf"},
            )
            response = connection.getresponse()
            payload = response.read()
            status = response.status
            if status == 200 and payload:
                rejected = ExportTraceServiceResponse.FromString(
                    payload
                ).partial_success.rejected_spans
        except (OSError, http.client.HTTPException):
            status = 0
            connection.close()
            connection = http.client.HTTPConnection("127.0.0.1", PORT, timeout=60)
        done = time.monotonic()
        previous_done = done
        records.append(
            (
                sent,
                done,
                status,
                spans,
                len(body),
                mine[slot][0],
                number,
                project,
                rejected,
                lag,
                waited,
            )
        )

        next_send += interval
    results.put(records)


# --- server --------------------------------------------------------------------------------------------


class Server:
    def __init__(self, mode: str, binary: Path, cores: int, memory: str):
        self.mode, self.binary, self.cores, self.memory = mode, binary, cores, memory
        # Whether the core count is enforced, not merely a worker-thread count: reported with every figure.
        self.pinned = mode == "container" or bool(shutil.which("taskset"))
        self.work = Path(tempfile.mkdtemp(prefix="sideseat-throughput-"))
        self.process = None
        self.container = f"sideseat-throughput-{os.getpid()}"

    def start(self) -> None:
        env = {
            "SIDESEAT_SECRETS_BACKEND": "file",
            "SIDESEAT_RATE_LIMIT_ENABLED": "false",
        }
        if self.mode == "local":
            env |= {
                "PATH": os.environ["PATH"],
                "HOME": str(self.work),
                "SIDESEAT_DATA_DIR": str(self.work),
                "SIDESEAT_PORT": str(PORT),
                "SIDESEAT_UI_PORT": str(PORT + 1),
                "SIDESEAT_OTEL_GRPC_PORT": str(PORT + 2),
            }
            if self.cores:
                env["TOKIO_WORKER_THREADS"] = str(self.cores)
            pin = (
                ["taskset", "-c", f"0-{self.cores - 1}"]
                if self.pinned and self.cores
                else []
            )
            self.process = subprocess.Popen(
                [*pin, str(self.binary), "--no-auth"],
                cwd=self.work,
                env=env,
                stdout=open(self.work / "server.log", "wb"),
                stderr=subprocess.STDOUT,
            )
        else:
            args = [
                "docker",
                "run",
                "-d",
                "--name",
                self.container,
                "-p",
                f"{PORT}:5388",
                f"--cpus={self.cores}",
                f"--cpuset-cpus=0-{self.cores - 1}",
                f"--memory={self.memory}",
                f"--memory-swap={self.memory}",
                "--tmpfs",
                "/tmp",
            ]
            for key, value in (env | {"TOKIO_WORKER_THREADS": str(self.cores)}).items():
                args += ["-e", f"{key}={value}"]
            subprocess.run([*args, IMAGE, "--no-auth"], check=True, capture_output=True)
        for _ in range(90):
            try:
                connection = http.client.HTTPConnection("127.0.0.1", PORT, timeout=2)
                connection.request("GET", "/api/v1/health")
                if connection.getresponse().status == 200:
                    return
            except OSError:
                pass
            time.sleep(1)
        sys.exit(f"[throughput] server did not start: {self.logs()[-2000:]}")

    def logs(self) -> str:
        if self.mode == "local":
            return (self.work / "server.log").read_text(errors="replace")
        return subprocess.run(
            ["docker", "logs", self.container], capture_output=True, text=True
        ).stderr

    def memory_bytes(self) -> int:
        if self.mode == "local":
            out = subprocess.run(
                ["ps", "-o", "rss=", "-p", str(self.process.pid)],
                capture_output=True,
                text=True,
            ).stdout.strip()
            return int(out or 0) * 1024
        out = subprocess.run(
            ["docker", "exec", self.container, "cat", "/sys/fs/cgroup/memory.current"],
            capture_output=True,
            text=True,
        ).stdout
        return int(out.strip() or 0)

    def disk_bytes(self) -> int:
        if self.mode == "local":
            return sum(p.stat().st_size for p in self.work.rglob("*") if p.is_file())
        out = subprocess.run(
            ["docker", "exec", self.container, "du", "-sb", "/data"],
            capture_output=True,
            text=True,
        ).stdout.split()
        return int(out[0]) if out else 0

    def projects(self, count: int) -> list[str]:
        projects = ["default"]
        for index in range(count - 1):
            connection = http.client.HTTPConnection("127.0.0.1", PORT, timeout=10)
            connection.request(
                "POST",
                "/api/v1/projects",
                json.dumps({"name": f"load-{index}", "organization_id": "default"}),
                {"Content-Type": "application/json"},
            )
            projects.append(json.loads(connection.getresponse().read())["id"])
        return projects

    def stored_spans(self, project: str) -> int:
        """Distinct spans the project stores: the span list's total, which counts each identity once."""
        connection = http.client.HTTPConnection("127.0.0.1", PORT, timeout=60)
        connection.request("GET", f"/api/v1/project/{project}/otel/spans?limit=1")
        response = connection.getresponse()
        body = response.read()
        if response.status != 200:
            sys.exit(
                f"[throughput] counting {project}'s spans answered {response.status}: {body[:300]!r}"
            )
        return int(json.loads(body)["meta"]["total_items"])

    def settled_spans(self, projects: list[str]) -> dict[str, int]:
        """Stored spans per project once the counts stop moving: a durable queue persists after the 200."""
        previous = None
        for _ in range(120):
            counts = {project: self.stored_spans(project) for project in projects}
            if counts == previous:
                return counts
            previous = counts
            time.sleep(1)
        return previous

    def stop(self) -> None:
        if self.mode == "local" and self.process:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=60)
            except subprocess.TimeoutExpired:
                self.process.kill()
        elif self.mode == "container":
            subprocess.run(["docker", "rm", "-fv", self.container], capture_output=True)
        subprocess.run(["rm", "-rf", str(self.work)])


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        return float("nan")
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(len(ordered) * fraction))]


def run_step(
    server: Server,
    pool_path: str,
    projects,
    rate: int,
    seconds: int,
    workers: int,
    step: int,
) -> tuple[dict, list]:
    results, ready = multiprocessing.Queue(), multiprocessing.Queue()
    go, start = multiprocessing.Event(), multiprocessing.Value("d", 0.0)
    processes = [
        multiprocessing.Process(
            target=worker,
            args=(
                i,
                workers,
                pool_path,
                projects,
                rate,
                seconds,
                step,
                ready,
                go,
                start,
                results,
            ),
        )
        for i in range(workers)
    ]
    for process in processes:
        process.start()
    for _ in processes:
        ready.get()
    start_at = time.monotonic() + 0.2
    start.value = start_at
    go.set()
    peak = 0
    while time.monotonic() < start_at + seconds:
        peak = max(peak, server.memory_bytes())
        time.sleep(0.5)
    records = [r for _ in processes for r in results.get()]
    for process in processes:
        process.join()
    finished = max((r[1] for r in records), default=start_at)
    ok = [r for r in records if r[2] == 200]
    failed = len(records) - len(ok)
    # 503 is back-pressure, 0 a timeout or dropped connection: both mean the offered rate is past capacity.
    failures = collections.Counter(str(r[2]) for r in records if r[2] != 200)
    rejected = sum(r[8] for r in ok)
    elapsed = max(finished, start_at + seconds) - start_at
    spans = sum(r[3] for r in ok) - rejected
    latencies = [(r[1] - r[0]) * 1000 for r in ok]
    lags = [r[9] * 1000 for r in records]
    # Requests that could not leave on schedule because their worker still waited on a response: the offered
    # load is capped at one request in flight per worker, so past this point the step under-offers.
    behind = sum(1 for r in records if r[10] > 0.010) / max(len(records), 1)
    # The schedule's own span count, not the nominal rate: requests carry 1 to 100+ spans, so at low rates the
    # few requests one step sends can sum to well under or over the nominal figure.
    scheduled = sum(r[3] for r in records) / seconds
    return {
        "offered_spans_per_s": rate,
        "scheduled_spans_per_s": scheduled,
        "achieved_spans_per_s": spans / elapsed,
        "requests": len(records),
        "failed": failed,
        "failures_by_status": dict(failures),
        "rejected_spans": rejected,
        "mb_per_s": sum(r[4] for r in ok) / elapsed / 1e6,
        "p50_ms": percentile(latencies, 0.5),
        "p99_ms": percentile(latencies, 0.99),
        "generator_p99_lag_ms": percentile(lags, 0.99),
        "in_flight_capped_fraction": behind,
        "peak_memory_bytes": peak,
    }, ok


def expected_identities(pool, accepted, step: int, into: dict) -> None:
    """Add the identities the accepted requests carried, per project, to `into`.

    Exports in the corpus can re-send a span another export already carried, so the expectation is a union, not
    a sum - exactly what the store keeps, since it holds one row per identity.
    """
    for record in accepted:
        template, number, project = record[5], record[6], record[7]
        into.setdefault(project, set()).update(
            keyed(pool[template][2], pass_key(step, number))
        )


def main() -> int:
    import pickle

    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("local", "container"))
    parser.add_argument("--cores", default="1", help="comma-separated core counts")
    parser.add_argument("--memory", default="2g")
    parser.add_argument(
        "--rates", default="250,500,1000,2500,5000,10000,25000,50000,100000"
    )
    parser.add_argument("--step-secs", type=int, default=20)
    parser.add_argument("--workers", type=int, default=16)
    parser.add_argument("--projects", type=int, default=8)
    parser.add_argument(
        "--min-sustained",
        type=float,
        help="fail the run unless every core count sustains at least this many spans/s",
    )
    parser.add_argument(
        "--no-media", action="store_true", help="leave out exports above 512 KB"
    )
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--json", type=Path)
    args = parser.parse_args()

    requests = load_requests(include_media=not args.no_media)
    pool = build_pool(requests)
    spans = sum(s for _, s, _ in pool)
    log(
        f"load: {len(pool)} exports, {spans} spans, {sum(len(b) for b, _, _ in pool) / 1e6:.1f} MB per pass, "
        "every pass with new ids"
    )
    pool_file = tempfile.NamedTemporaryFile(delete=False, suffix=".pickle")
    pool_file.close()
    mean_spans = spans / len(pool)
    for index in range(args.workers):
        share = [(i, body, n) for i, (body, n, _) in enumerate(pool)][
            index :: args.workers
        ]
        Path(f"{pool_file.name}.{index}").write_bytes(pickle.dumps((share, mean_spans)))
    binary = None
    if args.mode == "local":
        target = subprocess.run(
            ["bash", str(ROOT / "scripts/dev/cargo-target-dir.sh")],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
        binary = (args.binary or Path(target) / "release/sideseat").resolve()
    runs = []
    mismatch = False
    try:
        for cores in [int(c) for c in args.cores.split(",")]:
            server = Server(args.mode, binary, cores, args.memory)
            server.start()
            try:
                projects = server.projects(args.projects)
                steps = []
                expected: dict[str, set] = {}
                pinning = (
                    "pinned" if server.pinned else "NOT pinned (worker threads only)"
                )
                log(
                    f"{cores} core(s), {pinning}: offered -> achieved spans/s, p50/p99 ms, peak memory"
                )
                for number, rate in enumerate(int(r) for r in args.rates.split(",")):
                    step, accepted = run_step(
                        server,
                        pool_file.name,
                        projects,
                        rate,
                        args.step_secs,
                        args.workers,
                        number,
                    )
                    expected_identities(pool, accepted, number, expected)
                    stored = server.settled_spans(projects)
                    want = {project: len(ids) for project, ids in expected.items()}
                    agrees = all(
                        stored.get(project, 0) == want.get(project, 0)
                        for project in projects
                    )
                    step["stored_spans"] = sum(stored.values())
                    step["expected_spans"] = sum(want.values())
                    step["stored_matches_accepted"] = agrees
                    generator_bound = step["generator_p99_lag_ms"] > 50
                    step["generator_bound"] = generator_bound
                    passed = (
                        step["failed"] == 0
                        and step["rejected_spans"] == 0
                        and agrees
                        and step["achieved_spans_per_s"]
                        >= 0.95 * step["scheduled_spans_per_s"]
                    )
                    step["passed"] = passed
                    steps.append(step)
                    log(
                        f"  {rate:>7} ({step['scheduled_spans_per_s']:>7.0f}) -> {step['achieved_spans_per_s']:>9.0f}  "
                        f"{step['p50_ms']:7.1f} / {step['p99_ms']:7.1f}  "
                        f"{step['peak_memory_bytes'] / 2**20:6.0f} MB  "
                        f"{'ok' if passed else 'FAIL'} ({step['failed']} failed"
                        + (f" {step['failures_by_status']}" if step["failed"] else "")
                        + f", {step['rejected_spans']} rejected, "
                        f"stored {step['stored_spans']}/{step['expected_spans']})"
                        + ("  GENERATOR-BOUND" if generator_bound else "")
                        + (
                            f"  in-flight cap reached for {step['in_flight_capped_fraction']:.0%} of sends"
                            if step["in_flight_capped_fraction"] > 0.05
                            else ""
                        )
                    )
                    if not agrees:
                        mismatch = True
                        log(
                            "  stored spans disagree with accepted ones: the run is invalid"
                        )
                    if not passed:
                        break
                sustained = max(
                    (s["achieved_spans_per_s"] for s in steps if s["passed"]), default=0
                )
                disk = server.disk_bytes()
                log(
                    f"{cores} core(s): sustained {sustained:.0f} spans/s; {disk / 2**20:.0f} MB on disk"
                )
                runs.append(
                    {
                        "cores": cores,
                        "pinned": server.pinned,
                        "sustained_spans_per_s": sustained,
                        "steps": steps,
                        "disk_bytes": disk,
                    }
                )
            finally:
                server.stop()
    finally:
        os.unlink(pool_file.name)
        for index in range(args.workers):
            os.unlink(f"{pool_file.name}.{index}")
    if args.json:
        args.json.write_text(
            json.dumps(
                {"mode": args.mode, "memory": args.memory, "runs": runs}, indent=1
            )
        )
    if mismatch:
        return 1
    if args.min_sustained is not None:
        short = [r for r in runs if r["sustained_spans_per_s"] < args.min_sustained]
        for run in short:
            log(
                f"FAIL: {run['cores']} core(s) sustained {run['sustained_spans_per_s']:.0f} < {args.min_sustained:.0f}"
            )
        if short:
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
