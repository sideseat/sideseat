#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["opentelemetry-proto==1.45.0"]
# ///
"""A deterministic metric load derived from the captured metric exports, for measuring bytes per point.

The captured corpus (`server/tests/fixtures/metrics`) holds 480 points: enough to prove correctness, far too few to
measure storage, since one DuckDB block is 256 KB. This derives a load of the same *shapes* - every captured
series keeps its resource, scope, instrument kind, unit, temporality and attribute set - from a fleet of
`instances` copies of each producer (distinguished by `service.instance.id`, as a deployment's replicas are),
batched into one export per producer and interval as a collector sends them, cumulative every `interval_s` seconds for as many intervals as reach `points`. Values evolve as a
running service's do: counters and histogram buckets only grow, by seeded random increments.

Parameters live in `scripts/perf/metrics-load.json` and are committed instead of the data; the same parameters and
the same captured corpus give the same exports byte for byte.

    uv run --locked --script scripts/perf/metrics-load.py OUT_DIR
"""

from __future__ import annotations

import copy
import json
import random
import sys
from pathlib import Path

from opentelemetry.proto.collector.metrics.v1.metrics_service_pb2 import (
    ExportMetricsServiceRequest,
)
from opentelemetry.proto.common.v1.common_pb2 import AnyValue, KeyValue

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / "server/tests/fixtures/metrics"
PARAMETERS = ROOT / "scripts/perf/metrics-load.json"


def templates() -> dict[str, list]:
    """Per producer: one (resource, scope, metric) template per captured metric, last export wins."""
    by_tenant: dict[str, dict[tuple, tuple]] = {}
    for path in sorted(CORPUS.rglob("metrics-*.pb")):
        tenant = path.relative_to(CORPUS).parts[0]
        request = ExportMetricsServiceRequest()
        request.ParseFromString(path.read_bytes())
        for rm in request.resource_metrics:
            for sm in rm.scope_metrics:
                for metric in sm.metrics:
                    key = (sm.scope.name, metric.name)
                    by_tenant.setdefault(tenant, {})[key] = (
                        rm.resource,
                        sm.scope,
                        metric,
                    )
    return {tenant: list(found.values()) for tenant, found in sorted(by_tenant.items())}


def points_per_interval(metric) -> int:
    kind = metric.WhichOneof("data")
    return len(getattr(metric, kind).data_points) if kind else 0


def advance(metric, rng: random.Random, at_ns: int, start_ns: int) -> None:
    """Move every point of `metric` one interval on: later timestamp, cumulative values grown."""
    kind = metric.WhichOneof("data")
    for point in getattr(metric, kind).data_points:
        point.time_unix_nano = at_ns
        point.start_time_unix_nano = start_ns
        if kind == "sum":
            if point.HasField("as_int"):
                point.as_int += rng.randint(0, 40)
            else:
                point.as_double += rng.random() * 5.0
        elif kind == "gauge":
            if point.HasField("as_int"):
                point.as_int = max(0, point.as_int + rng.randint(-3, 3))
            else:
                point.as_double = max(0.0, point.as_double + rng.uniform(-1, 1))
        elif kind == "histogram":
            added = rng.randint(0, 6)
            for _ in range(added):
                bucket = rng.randrange(max(len(point.bucket_counts), 1))
                if point.bucket_counts:
                    point.bucket_counts[bucket] += 1
                value = rng.expovariate(1 / 400.0)
                point.sum += value
                point.min = min(point.min, value) if point.count else value
                point.max = max(point.max, value)
                point.count += 1
        elif kind == "exponential_histogram":
            added = rng.randint(0, 6)
            for _ in range(added):
                if point.positive.bucket_counts:
                    point.positive.bucket_counts[
                        rng.randrange(len(point.positive.bucket_counts))
                    ] += 1
                value = rng.expovariate(1 / 400.0)
                point.sum += value
                point.count += 1


def generate(out: Path) -> int:
    """Write the exports; returns how many points they hold."""
    parameters = json.loads(PARAMETERS.read_text())
    rng = random.Random(parameters["seed"])
    interval_ns = parameters["interval_s"] * 1_000_000_000
    start_ns = parameters["start_unix_s"] * 1_000_000_000
    found = templates()
    per_interval = sum(
        points_per_interval(metric) * parameters["instances"]
        for series in found.values()
        for (_, _, metric) in series
    )
    intervals = -(-parameters["points"] // per_interval)
    out.mkdir(parents=True, exist_ok=True)
    written = 0
    for tenant, series in found.items():
        fleet = []
        for instance in range(parameters["instances"]):
            states = []
            for resource, scope, metric in series:
                resource = copy.deepcopy(resource)
                resource.attributes.append(
                    KeyValue(
                        key="service.instance.id",
                        value=AnyValue(string_value=f"{tenant}-{instance:03d}"),
                    )
                )
                states.append((resource, scope, copy.deepcopy(metric)))
            fleet.append(states)
        # One export per interval for the whole fleet, as a collector batching its replicas sends it.
        for step in range(intervals):
            at_ns = start_ns + (step + 1) * interval_ns
            request = ExportMetricsServiceRequest()
            for states in fleet:
                groups: dict[bytes, object] = {}
                for resource, scope, metric in states:
                    advance(metric, rng, at_ns, start_ns)
                    key = resource.SerializeToString()
                    if key not in groups:
                        rm = request.resource_metrics.add()
                        rm.resource.CopyFrom(resource)
                        groups[key] = rm
                    sm = groups[key].scope_metrics.add()
                    sm.scope.CopyFrom(scope)
                    sm.metrics.add().CopyFrom(metric)
                    written += points_per_interval(metric)
            name = out / tenant / f"metrics-{step:06d}.pb"
            name.parent.mkdir(parents=True, exist_ok=True)
            name.write_bytes(request.SerializeToString())
    return written


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    count = generate(Path(sys.argv[1]))
    print(f"[metrics-load] {count} points written to {sys.argv[1]}")
