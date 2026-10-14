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
batched into one export per producer and interval as a collector sends them, cumulative every `interval_s` seconds
for as many intervals as reach `points`. Values evolve as a running service's do: counters and histogram buckets
only grow, by seeded random increments, and gauges wander. Nothing is repeated that a real fleet would vary: each
replica started at its own instant, up to `start_spread_s` before the load, which every cumulative point carries,
and collects at its own phase of the interval, each collection late by up to `jitter_ms` - so no two replicas'
timestamps agree, and none repeats an interval exactly. Every series any capture carried is kept, and each point
carries one of the exemplar sets its series' captured points carried, chosen at random, so exemplars arrive as often
as they did in the captures - each with a fresh trace, span, instant and value.

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
# `exemplars: "densest"` gives every point the most exemplars its series was captured with, about twice the captured
# rate, to show a figure does not depend on exemplars being sparse; the default, "captured", samples the captures.
DENSEST = json.loads(PARAMETERS.read_text()).get("exemplars", "captured") == "densest"


def series_key(point) -> bytes:
    """A data point's series within its metric: its attribute set, in a stable order."""
    return b"".join(
        kv.SerializeToString() for kv in sorted(point.attributes, key=lambda kv: kv.key)
    )


def templates() -> dict[str, list]:
    """Per producer: one (resource, scope, metric, samples) template per captured resource, scope and metric, holding
    every series any export of it carried - its latest point - and, per series, the exemplar sets its captured points
    carried, one of which each generated point carries."""
    by_tenant: dict[str, dict[tuple, tuple]] = {}
    for path in sorted(CORPUS.rglob("metrics-*.pb")):
        tenant = path.relative_to(CORPUS).parts[0]
        request = ExportMetricsServiceRequest()
        request.ParseFromString(path.read_bytes())
        for rm in request.resource_metrics:
            # The capture's own instance id gives way to the replica's, which the fleet adds.
            resource = copy.deepcopy(rm.resource)
            kept = [kv for kv in resource.attributes if kv.key != "service.instance.id"]
            del resource.attributes[:]
            resource.attributes.extend(kept)
            for sm in rm.scope_metrics:
                for metric in sm.metrics:
                    kind = metric.WhichOneof("data")
                    if not kind:
                        continue
                    # A series is its resource, scope and metric as well as its attributes: two processes'
                    # resources differ (`process.pid`) for the same metric and attribute set.
                    key = (
                        resource.SerializeToString(deterministic=True),
                        sm.scope.SerializeToString(deterministic=True),
                        metric.name,
                    )
                    found = by_tenant.setdefault(tenant, {})
                    _, _, _, points, samples = found.get(
                        key, (None, None, None, {}, {})
                    )
                    for point in getattr(metric, kind).data_points:
                        series = series_key(point)
                        points[series] = copy.deepcopy(point)
                        samples.setdefault(series, []).append(list(point.exemplars))
                    found[key] = (resource, sm.scope, metric, points, samples)
    merged: dict[str, list] = {}
    for tenant, found in sorted(by_tenant.items()):
        merged[tenant] = []
        for resource, scope, metric, points, samples in found.values():
            metric = copy.deepcopy(metric)
            data = getattr(metric, metric.WhichOneof("data"))
            del data.data_points[:]
            data.data_points.extend(points.values())
            merged[tenant].append(
                (resource, scope, metric, [samples[k] for k in points])
            )
    return merged


def fresh_exemplars(
    point, sampled: list, rng: random.Random, at_ns: int, interval_ns: int
) -> None:
    """Give the point one of its series' captured exemplar sets, each exemplar with a new trace, span, instant and
    value within the interval just ended, as a running service's exemplars name the requests it last sampled; their
    filtered attributes stay the captured ones."""
    del point.exemplars[:]
    chosen = max(sampled, key=len) if DENSEST else rng.choice(sampled)
    point.exemplars.extend(copy.deepcopy(chosen))
    for exemplar in point.exemplars:
        exemplar.trace_id = rng.randbytes(16)
        exemplar.span_id = rng.randbytes(8)
        exemplar.time_unix_nano = at_ns - rng.randrange(interval_ns)
        if exemplar.HasField("as_int"):
            exemplar.as_int = rng.randint(0, 2_000)
        else:
            exemplar.as_double = rng.expovariate(1 / 400.0)


def points_per_interval(metric) -> int:
    kind = metric.WhichOneof("data")
    return len(getattr(metric, kind).data_points) if kind else 0


def advance(
    metric,
    samples: list,
    rng: random.Random,
    at_ns: int,
    start_ns: int,
    interval_ns: int,
) -> None:
    """Move every point of `metric` one interval on: later timestamp, cumulative values grown, fresh exemplars."""
    kind = metric.WhichOneof("data")
    for point, sampled in zip(getattr(metric, kind).data_points, samples):
        point.time_unix_nano = at_ns
        point.start_time_unix_nano = start_ns
        fresh_exemplars(point, sampled, rng, at_ns, interval_ns)
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
        for (_, _, metric, _) in series
    )
    intervals = -(-parameters["points"] // per_interval)
    out.mkdir(parents=True, exist_ok=True)
    written = 0
    spread_ns = parameters["start_spread_s"] * 1_000_000_000
    jitter_ns = parameters["jitter_ms"] * 1_000_000
    for tenant, series in found.items():
        fleet = []
        # Per replica: the instant it started, which its cumulative points carry, and its phase in the interval.
        clocks = [
            (start_ns - rng.randrange(spread_ns + 1), rng.randrange(interval_ns))
            for _ in range(parameters["instances"])
        ]
        for instance in range(parameters["instances"]):
            states = []
            for resource, scope, metric, samples in series:
                resource = copy.deepcopy(resource)
                resource.attributes.append(
                    KeyValue(
                        key="service.instance.id",
                        value=AnyValue(string_value=f"{tenant}-{instance:03d}"),
                    )
                )
                states.append((resource, scope, copy.deepcopy(metric), samples))
            fleet.append(states)
        # One export per interval for the whole fleet, as a collector batching its replicas sends it.
        for step in range(intervals):
            request = ExportMetricsServiceRequest()
            for states, (started_ns, phase_ns) in zip(fleet, clocks):
                at_ns = (
                    start_ns
                    + step * interval_ns
                    + phase_ns
                    + rng.randrange(jitter_ns + 1)
                )
                groups: dict[bytes, object] = {}
                for resource, scope, metric, samples in states:
                    advance(metric, samples, rng, at_ns, started_ns, interval_ns)
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
