"""
Generate 1M spans from a template JSONL file for performance testing.

Reads traces from a gzipped JSONL file, remaps trace/span IDs, shifts
timestamps across a configurable time range, and sends them to the server
using concurrent HTTP requests with batched OTLP payloads.

Handles server backpressure (503) with exponential backoff and retry.

Usage:
    uv run --directory misc/replay generate_load \
        --spans 1000000 \
        --workers 32 \
        --spans-per-request 50
"""

import argparse
import gzip
import json
import os
import random
import re
import sys
import time
import uuid
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from queue import Empty, Queue
from threading import Lock

import requests
from dotenv import load_dotenv

_misc_dir = Path(__file__).parent.parent
load_dotenv(_misc_dir / ".env")

FIXTURES_DIR = _misc_dir / "fixtures"
HEADERS = {"Content-Type": "application/json"}

MAX_RETRIES = 8
INITIAL_BACKOFF_S = 0.5
MAX_BACKOFF_S = 30.0


def get_base_url() -> str:
    endpoint = os.getenv(
        "OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:5388/otel/default"
    )
    match = re.match(r"(https?://[^/]+)", endpoint)
    return match.group(1) if match else "http://127.0.0.1:5388"


def load_template_traces(path: Path) -> dict[str, list[dict]]:
    """Load and group raw resourceSpan objects by trace_id.

    Returns {trace_id: [resourceSpan_dicts]} where each resourceSpan
    contains exactly one span (as found in the source file).
    """
    traces: dict[str, list[dict]] = defaultdict(list)

    opener = gzip.open if path.suffix == ".gz" else open
    with opener(path, "rt", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            entry = json.loads(line)
            for rs in entry["data"].get("resourceSpans", []):
                for ss in rs.get("scopeSpans", []):
                    for span in ss.get("spans", []):
                        traces[span["traceId"]].append(rs)
                        break
                    break
                break

    return dict(traces)


def new_hex_id(length: int) -> str:
    return uuid.uuid4().hex[:length]


def remap_resource_span(
    rs: dict,
    old_trace_id: str,
    new_trace_id: str,
    span_id_map: dict[str, str],
    time_offset_ns: int,
    project_id: str,
) -> dict:
    """Remap a single resourceSpan with new IDs and shifted timestamps."""
    resource = rs.get("resource", {})
    attrs = resource.get("attributes", [])
    new_attrs = []
    for attr in attrs:
        if attr.get("key") == "sideseat.project_id":
            new_attrs.append(
                {"key": "sideseat.project_id", "value": {"stringValue": project_id}}
            )
        else:
            new_attrs.append(attr)
    new_resource = {**resource, "attributes": new_attrs}

    new_ss_list = []
    for ss in rs.get("scopeSpans", []):
        new_spans = []
        for span in ss.get("spans", []):
            new_span = dict(span)
            new_span["traceId"] = new_trace_id

            old_sid = span["spanId"]
            if old_sid not in span_id_map:
                span_id_map[old_sid] = new_hex_id(16)
            new_span["spanId"] = span_id_map[old_sid]

            if span.get("parentSpanId"):
                old_parent = span["parentSpanId"]
                if old_parent not in span_id_map:
                    span_id_map[old_parent] = new_hex_id(16)
                new_span["parentSpanId"] = span_id_map[old_parent]

            if "startTimeUnixNano" in span:
                new_span["startTimeUnixNano"] = str(
                    int(span["startTimeUnixNano"]) + time_offset_ns
                )
            if "endTimeUnixNano" in span:
                new_span["endTimeUnixNano"] = str(
                    int(span["endTimeUnixNano"]) + time_offset_ns
                )

            if "events" in span:
                new_events = []
                for event in span["events"]:
                    new_event = dict(event)
                    if "timeUnixNano" in event:
                        new_event["timeUnixNano"] = str(
                            int(event["timeUnixNano"]) + time_offset_ns
                        )
                    new_events.append(new_event)
                new_span["events"] = new_events

            new_spans.append(new_span)
        new_ss_list.append({**ss, "spans": new_spans})
    return {**rs, "resource": new_resource, "scopeSpans": new_ss_list}


def send_request(
    session: requests.Session, url: str, resource_spans: list[dict],
    stats: "Stats",
) -> tuple[int, int]:
    """Send a batched OTLP request with retry on 503.

    Returns (spans_sent, errors).
    """
    payload = {"resourceSpans": resource_spans}
    backoff = INITIAL_BACKOFF_S

    for attempt in range(MAX_RETRIES + 1):
        try:
            resp = session.post(url, json=payload, timeout=60)
            if resp.status_code == 503:
                if attempt < MAX_RETRIES:
                    with stats.lock:
                        stats.retries += 1
                    jitter = random.uniform(0, backoff * 0.5)
                    time.sleep(backoff + jitter)
                    backoff = min(backoff * 2, MAX_BACKOFF_S)
                    continue
                print(f"\n  [ERR] 503 after {MAX_RETRIES} retries", flush=True)
                return 0, len(resource_spans)
            resp.raise_for_status()
            return len(resource_spans), 0
        except requests.ConnectionError as e:
            if attempt < MAX_RETRIES:
                with stats.lock:
                    stats.retries += 1
                jitter = random.uniform(0, backoff * 0.5)
                time.sleep(backoff + jitter)
                backoff = min(backoff * 2, MAX_BACKOFF_S)
                continue
            print(f"\n  [ERR] Connection failed after {MAX_RETRIES} retries: {e}", flush=True)
            return 0, len(resource_spans)
        except requests.RequestException as e:
            print(f"\n  [ERR] HTTP {resp.status_code if 'resp' in dir() else '?'}: {e}", flush=True)
            return 0, len(resource_spans)

    return 0, len(resource_spans)


class Stats:
    def __init__(self):
        self.lock = Lock()
        self.spans_sent = 0
        self.spans_errored = 0
        self.requests_sent = 0
        self.retries = 0


def worker_fn(
    work_queue: Queue,
    url: str,
    stats: Stats,
    spans_per_request: int,
):
    """Worker that pulls remapped resourceSpans from queue and sends batched requests."""
    session = requests.Session()
    session.headers.update(HEADERS)
    batch: list[dict] = []

    while True:
        try:
            item = work_queue.get(timeout=3)
        except Empty:
            break

        if item is None:
            work_queue.task_done()
            break

        batch.append(item)
        work_queue.task_done()

        if len(batch) >= spans_per_request:
            sent, errs = send_request(session, url, batch, stats)
            with stats.lock:
                stats.spans_sent += sent
                stats.spans_errored += errs
                stats.requests_sent += 1
            batch = []

    # Flush remaining partial batch
    if batch:
        sent, errs = send_request(session, url, batch, stats)
        with stats.lock:
            stats.spans_sent += sent
            stats.spans_errored += errs
            stats.requests_sent += 1

    session.close()


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate load test spans")
    parser.add_argument(
        "--source",
        default="traces-strands.jsonl.gz",
        help="Source JSONL file (relative to fixtures/ or absolute)",
    )
    parser.add_argument(
        "--spans",
        type=int,
        default=1_000_000,
        help="Target number of spans to generate (default: 1000000)",
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=32,
        help="Concurrent HTTP workers (default: 32)",
    )
    parser.add_argument(
        "--spans-per-request",
        type=int,
        default=25,
        help="ResourceSpans per OTLP request (default: 25)",
    )
    parser.add_argument(
        "--spread-days",
        type=int,
        default=30,
        help="Spread traces across N days (default: 30)",
    )
    parser.add_argument("--base-url", help="Server base URL")
    parser.add_argument(
        "--project-id",
        default="default",
        help="Project ID (default: default)",
    )
    args = parser.parse_args()

    # Resolve source path
    source = Path(args.source)
    if not source.is_absolute():
        source = FIXTURES_DIR / args.source
    if not source.exists():
        print(f"Error: File not found: {source}", file=sys.stderr)
        return 1

    base_url = args.base_url or get_base_url()
    url = f"{base_url}/otel/{args.project_id}/v1/traces"

    # Load templates
    print("Loading template traces...")
    templates = load_template_traces(source)
    template_ids = list(templates.keys())
    template_count = len(template_ids)
    total_template_spans = sum(len(v) for v in templates.values())
    avg_spans = total_template_spans / template_count

    print(
        f"Loaded {template_count} template traces "
        f"({total_template_spans} spans, avg {avg_spans:.1f}/trace)"
    )
    print(f"Target spans: {args.spans:,}")
    print()
    print(f"Source: {source}")
    print(f"Target: {url}")
    print(f"Workers: {args.workers}")
    print(f"Spans/request: {args.spans_per_request}")
    print(f"Spread: {args.spread_days} days")
    print(f"Retry: up to {MAX_RETRIES} attempts with exponential backoff")
    print()

    # Find max timestamp in templates
    max_template_ts = 0
    for entries in templates.values():
        for rs in entries:
            for ss in rs.get("scopeSpans", []):
                for span in ss.get("spans", []):
                    ts = int(span.get("endTimeUnixNano", "0"))
                    max_template_ts = max(max_template_ts, ts)

    spread_ns = args.spread_days * 24 * 3600 * 1_000_000_000
    now_ns = int(time.time() * 1_000_000_000)

    # Set up work queue and workers
    stats = Stats()
    work_queue: Queue = Queue(maxsize=args.workers * args.spans_per_request * 2)

    print("Starting workers...")
    executor = ThreadPoolExecutor(max_workers=args.workers)
    futures = []
    for _ in range(args.workers):
        f = executor.submit(worker_fn, work_queue, url, stats, args.spans_per_request)
        futures.append(f)

    # Generate and enqueue remapped spans, stopping when we hit the target
    print("Generating and sending spans...")
    start_time = time.time()
    last_print = start_time
    spans_enqueued = 0
    traces_generated = 0
    i = 0

    while spans_enqueued < args.spans:
        template_id = template_ids[i % template_count]
        template_rs_list = templates[template_id]

        new_trace_id = new_hex_id(32)
        span_id_map: dict[str, str] = {}

        progress_frac = spans_enqueued / max(args.spans - 1, 1)
        time_offset_ns = (
            now_ns - max_template_ts - spread_ns + int(progress_frac * spread_ns)
        )

        for rs in template_rs_list:
            if spans_enqueued >= args.spans:
                break
            remapped = remap_resource_span(
                rs, template_id, new_trace_id, span_id_map, time_offset_ns,
                args.project_id,
            )
            work_queue.put(remapped)
            spans_enqueued += 1

        traces_generated += 1
        i += 1

        # Progress reporting
        now = time.time()
        if now - last_print >= 2.0:
            elapsed = now - start_time
            with stats.lock:
                sent = stats.spans_sent
                errs = stats.spans_errored
                reqs = stats.requests_sent
            rate = sent / elapsed if elapsed > 0 else 0
            print(
                f"\r  Enqueued: {spans_enqueued:>10,} / {args.spans:,}  |  "
                f"Sent: {sent:>10,}  |  "
                f"Errors: {errs:>8,}  |  "
                f"Requests: {reqs:>8,}  |  "
                f"{rate:>8,.0f} spans/s",
                end="",
                flush=True,
            )
            last_print = now

    # Send poison pills to stop workers
    for _ in range(args.workers):
        work_queue.put(None)

    # Wait for workers to finish
    for f in as_completed(futures):
        f.result()
    executor.shutdown(wait=True)

    elapsed = time.time() - start_time
    print()
    print()
    print(f"Done: {stats.spans_sent:,} spans sent, {stats.spans_errored:,} errors")
    print(f"Traces generated: {traces_generated:,}")
    print(f"Total HTTP requests: {stats.requests_sent:,}")
    if stats.retries > 0:
        print(f"Total retries: {stats.retries:,}")
    print(f"Elapsed: {elapsed:.1f}s")
    if elapsed > 0:
        print(f"Average: {stats.spans_sent / elapsed:,.0f} spans/s")

    return 1 if stats.spans_errored > 0 else 0


if __name__ == "__main__":
    sys.exit(main())
