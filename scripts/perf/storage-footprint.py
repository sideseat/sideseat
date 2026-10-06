#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["duckdb==1.5.2", "opentelemetry-proto==1.45.0"]
# ///
"""What the real backends store for the whole capture corpus, per signal, against raw OTLP protobuf.

`storage-entropy.py` estimates what a format *could* reach; this measures what a running server *does*
store. It starts the release server, posts every committed fixture export to it, stops it, and then reads
the backends' own accounting:

    uv run --locked --script scripts/perf/storage-footprint.py embedded      # DuckDB + SQLite + files
    uv run --locked --script scripts/perf/storage-footprint.py distributed   # ClickHouse + PostgreSQL + MinIO

The distributed mode starts throwaway containers; hold the container lock while it runs.

A tenant is one producer (the first path component under `server/tests/fixtures/messages`): every version,
mode and scenario of one framework is one application run many times, which is what a hosted project is.
Each gets its own project, so deduplication never crosses a project boundary - the store may not do that.

The denominator is the protobuf encoding of every export. A fixture captured as OTLP/JSON is converted to
protobuf for the denominator, because JSON is larger and would flatter the ratio.

Attribution is by owner, not by guess: analytics tables and term indexes go to their signal; content bodies
and extracted files belong to spans (the only signal that has them today); registry rows in the
transactional store are counted with the signal whose rows they reference. What cannot be attributed -
schema version rows, the pricing catalogue, users, projects - is reported as `fixed` and left out of the
ratio, because it does not grow with telemetry.

The gated figure is **stored bytes per item excluding media**: media (images, documents, audio) is stored once per
project, losslessly, at its floor - the unique decoded bytes - and is reported beside it, not inside it.
`--gate` fails the run when any signal exceeds its ceiling in `scripts/perf/storage-footprint-ceiling.json`: the
`target` the product promises, and a `regression` ceiling - the last measured figure plus a margin - so a change
that makes storage worse fails even while the target is still out of reach.
"""

from __future__ import annotations

import argparse
import collections
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / "server/tests/fixtures/messages"
# Metric exports have no message golden, so they live in their own corpus (`harness capture --metrics`).
METRIC_CORPUS = ROOT / "server/tests/fixtures/metrics"
CEILING = ROOT / "scripts/perf/storage-footprint-ceiling.json"
# Layers that are media objects. Everything else, content-body copies included, is telemetry encoding.
MEDIA_LAYERS = ("blobs:files",)
SIGNALS = ("traces", "logs", "metrics")
SPAN_TABLES = {
    "otel_spans": "traces",
    "span_terms": "traces",
    "span_partition_anomalies": "traces",
}
LOG_TABLES = {"otel_logs": "logs", "log_terms": "logs"}
METRIC_TABLES = {"otel_metrics": "metrics"}
ANALYTICS_OWNER = {**SPAN_TABLES, **LOG_TABLES, **METRIC_TABLES}
# Transactional tables whose rows exist per stored span or file. Everything else there is fixed.
TRANSACTIONAL_OWNER = {
    "content_bodies": "traces",
    "span_bodies": "traces",
    "files": "traces",
    "trace_files": "traces",
    "content_body_backfill": "traces",
}
PORT = int(os.environ.get("FOOTPRINT_STORAGE_PORT", "5621"))
SCOPE = str(abs(hash(str(ROOT))) % 1_000_000)
CH_NAME, PG_NAME, MINIO_NAME = (
    f"sideseat-storage-{n}-{SCOPE}" for n in ("ch", "pg", "minio")
)
CH_IMAGE = os.environ.get("CH_IMAGE_TAG", "26.4.3.37")


def log(message: str) -> None:
    print(f"[storage] {message}", flush=True)


# --- corpus ------------------------------------------------------------------------------------------


def corpus() -> list[dict]:
    """Every export in the corpus, with its signal, tenant and protobuf size."""
    from google.protobuf import json_format
    from opentelemetry.proto.collector.logs.v1.logs_service_pb2 import (
        ExportLogsServiceRequest,
    )
    from opentelemetry.proto.collector.metrics.v1.metrics_service_pb2 import (
        ExportMetricsServiceRequest,
    )
    from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import (
        ExportTraceServiceRequest,
    )

    kinds = {
        "req": ("traces", ExportTraceServiceRequest),
        "logs": ("logs", ExportLogsServiceRequest),
        "metrics": ("metrics", ExportMetricsServiceRequest),
    }
    exports = []
    for path in sorted([*CORPUS.rglob("*"), *METRIC_CORPUS.rglob("*")]):
        prefix = path.name.split("-", 1)[0]
        if (
            prefix not in kinds
            or path.suffix not in (".pb", ".json")
            or not path.is_file()
        ):
            continue
        signal_name, message_type = kinds[prefix]
        body = path.read_bytes()
        message = message_type()
        if path.suffix == ".json":
            json_format.Parse(body, message)
        else:
            message.ParseFromString(body)
        exports.append(
            {
                "path": path,
                "signal": signal_name,
                "tenant": path.relative_to(
                    METRIC_CORPUS if path.is_relative_to(METRIC_CORPUS) else CORPUS
                ).parts[0],
                "body": body,
                "json": path.suffix == ".json",
                "protobuf_bytes": message.ByteSize(),
                "items": count_items(signal_name, message),
            }
        )
    return exports


def count_items(signal_name: str, message) -> int:
    if signal_name == "traces":
        return sum(
            len(ss.spans) for rs in message.resource_spans for ss in rs.scope_spans
        )
    if signal_name == "logs":
        return sum(
            len(sl.log_records) for rl in message.resource_logs for sl in rl.scope_logs
        )
    points = 0
    for rm in message.resource_metrics:
        for sm in rm.scope_metrics:
            for metric in sm.metrics:
                kind = metric.WhichOneof("data")
                points += len(getattr(metric, kind).data_points) if kind else 0
    return points


# --- server lifecycle ----------------------------------------------------------------------------------


def http(method: str, url: str, body: bytes | None = None, headers: dict | None = None):
    request = urllib.request.Request(
        url, data=body, method=method, headers=headers or {}
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def docker(*args: str, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["docker", *args], check=check, capture_output=True, text=True
    )


def start_distributed(work: Path) -> dict:
    for name in (CH_NAME, PG_NAME, MINIO_NAME):
        docker("rm", "-fv", name, check=False)
    docker(
        "run",
        "-d",
        "--name",
        PG_NAME,
        "-p",
        "5452:5432",
        "-e",
        "POSTGRES_USER=sideseat",
        "-e",
        "POSTGRES_PASSWORD=sideseat",
        "-e",
        "POSTGRES_DB=sideseat",
        "postgres:17-alpine",
    )
    docker(
        "run",
        "-d",
        "--name",
        CH_NAME,
        "-p",
        "8141:8123",
        "-e",
        "CLICKHOUSE_USER=sideseat",
        "-e",
        "CLICKHOUSE_PASSWORD=sideseat",
        "-e",
        "CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT=1",
        f"clickhouse/clickhouse-server:{CH_IMAGE}",
    )
    docker(
        "run",
        "-d",
        "--name",
        MINIO_NAME,
        "-p",
        "9020:9000",
        "-e",
        "MINIO_ROOT_USER=sideseat",
        "-e",
        "MINIO_ROOT_PASSWORD=sideseat12345",
        "quay.io/minio/minio:RELEASE.2025-04-22T22-12-26Z",
        "server",
        "/data",
    )
    for _ in range(120):
        ready = (
            docker(
                "exec", PG_NAME, "pg_isready", "-U", "sideseat", check=False
            ).returncode
            == 0
        )
        if ready and http("GET", "http://127.0.0.1:8141/ping")[0] == 200:
            try:
                if http("GET", "http://127.0.0.1:9020/minio/health/live")[0] == 200:
                    break
            except OSError:
                pass
        time.sleep(1)
    else:
        sys.exit("[storage] the containers did not become ready")
    clickhouse("CREATE DATABASE IF NOT EXISTS sideseat")
    subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--network",
            "host",
            "--entrypoint",
            "sh",
            "quay.io/minio/mc:RELEASE.2025-04-16T18-13-26Z",
            "-c",
            "mc alias set s http://127.0.0.1:9020 sideseat sideseat12345 >/dev/null && "
            "mc mb --ignore-existing s/sideseat-storage >/dev/null",
        ],
        check=True,
        capture_output=True,
    )
    config = {
        "files": {
            "enabled": True,
            "storage": "s3",
            "s3": {
                "bucket": "sideseat-storage",
                "prefix": "files",
                "region": "us-east-1",
                "endpoint": "http://127.0.0.1:9020",
            },
        },
        "database": {
            "transactional": "postgres",
            "analytics": "clickhouse",
            "postgres": {"url": "postgres://sideseat:sideseat@127.0.0.1:5452/sideseat"},
            "clickhouse": {
                "url": "http://127.0.0.1:8141",
                "user": "sideseat",
                "password": "sideseat",
                "database": "sideseat",
            },
        },
    }
    (work / "sideseat.json").write_text(json.dumps(config))
    return {
        "AWS_ACCESS_KEY_ID": "sideseat",
        "AWS_SECRET_ACCESS_KEY": "sideseat12345",
        "AWS_REGION": "us-east-1",
    }


def stop_distributed() -> None:
    for name in (CH_NAME, PG_NAME, MINIO_NAME):
        docker("rm", "-fv", name, check=False)


def clickhouse(query: str) -> str:
    status, body = http(
        "POST",
        "http://127.0.0.1:8141/",
        query.encode(),
        {"X-ClickHouse-User": "sideseat", "X-ClickHouse-Key": "sideseat"},
    )
    if status != 200:
        raise RuntimeError(f"clickhouse: {body.decode(errors='replace')[:500]}")
    return body.decode()


def start_server(binary: Path, work: Path, extra_env: dict) -> subprocess.Popen:
    env = {
        "PATH": os.environ["PATH"],
        "HOME": str(work),
        "SIDESEAT_DATA_DIR": str(work),
        "SIDESEAT_SECRETS_BACKEND": "file",
        "SIDESEAT_PORT": str(PORT),
        "SIDESEAT_UI_PORT": str(PORT + 1),
        "SIDESEAT_OTEL_GRPC_PORT": str(PORT + 2),
        "SIDESEAT_RATE_LIMIT_ENABLED": "false",
        **extra_env,
    }
    server = subprocess.Popen(
        [str(binary), "--no-auth"],
        cwd=work,
        env=env,
        stdout=open(work / "server.log", "wb"),
        stderr=subprocess.STDOUT,
    )
    for _ in range(90):
        try:
            if http("GET", f"http://127.0.0.1:{PORT}/api/v1/health")[0] == 200:
                return server
        except OSError:
            pass
        if server.poll() is not None:
            break
        time.sleep(1)
    sys.exit(
        f"[storage] server did not start:\n{(work / 'server.log').read_text()[-3000:]}"
    )


def stop_server(server: subprocess.Popen) -> None:
    server.send_signal(signal.SIGTERM)
    try:
        server.wait(timeout=60)
    except subprocess.TimeoutExpired:
        server.kill()
        server.wait()


def load(exports: list[dict]) -> dict[str, str]:
    """One project per tenant; every export posted once. Returns tenant -> project id."""
    projects = {}
    base = f"http://127.0.0.1:{PORT}"
    for tenant in sorted({e["tenant"] for e in exports}):
        status, body = http(
            "POST",
            f"{base}/api/v1/projects",
            json.dumps({"name": tenant[:100], "organization_id": "default"}).encode(),
            {"Content-Type": "application/json"},
        )
        if status not in (200, 201):
            sys.exit(
                f"[storage] could not create project {tenant}: {status} {body[:200]!r}"
            )
        projects[tenant] = json.loads(body)["id"]
    started = time.monotonic()
    for export in exports:
        content_type = (
            "application/json" if export["json"] else "application/x-protobuf"
        )
        url = f"{base}/otel/{projects[export['tenant']]}/v1/{export['signal']}"
        status, body = http("POST", url, export["body"], {"Content-Type": content_type})
        if status != 200:
            sys.exit(
                f"[storage] {export['path'].relative_to(ROOT)} returned {status}: {body[:300]!r}"
            )
    log(f"posted {len(exports)} exports in {time.monotonic() - started:.1f}s")
    return projects


def settle(projects: dict[str, str]) -> None:
    """Wait until every project's logical storage stops changing: ingestion is asynchronous past the ack."""
    base = f"http://127.0.0.1:{PORT}"
    previous, stable = None, 0
    for _ in range(180):
        totals = []
        for project in projects.values():
            status, body = http("GET", f"{base}/api/v1/projects/{project}/storage")
            totals.append(
                json.loads(body).get("logical_bytes") if status == 200 else None
            )
        stable = stable + 1 if totals == previous else 0
        previous = totals
        if stable >= 5:
            return
        time.sleep(1)
    log("warning: storage accounting did not settle in 180s")


# --- measurement ---------------------------------------------------------------------------------------


def duckdb_columns(path: Path) -> tuple[dict, int]:
    """Bytes per (table, column) from DuckDB's segment map, and the file's used bytes.

    DuckDB does not report a segment's size, only where it starts. A segment's extent is therefore the gap to
    the next segment in the same block (or to the block end), plus any overflow blocks it owns. The sum is the
    used blocks, so the per-column split is approximate while the total is exact.
    """
    import duckdb

    connection = duckdb.connect(str(path), read_only=True)
    block_size = connection.execute(
        "SELECT block_size FROM pragma_database_size()"
    ).fetchone()[0]
    used = connection.execute(
        "SELECT used_blocks FROM pragma_database_size()"
    ).fetchone()[0]
    tables = [
        row[0]
        for row in connection.execute(
            "SELECT table_name FROM duckdb_tables()"
        ).fetchall()
    ]
    segments = []
    for table in tables:
        rows = connection.execute(
            f"SELECT column_name, block_id, block_offset, additional_block_ids "
            f"FROM pragma_storage_info('{table}') WHERE persistent AND block_id >= 0"
        ).fetchall()
        segments += [(table, *row) for row in rows]
    by_block = collections.defaultdict(list)
    for table, column, block, offset, extra in segments:
        by_block[block].append((offset, table, column, len(extra or [])))
    sizes = collections.Counter()
    header = 8  # DuckDB block checksum
    for entries in by_block.values():
        entries.sort()
        for index, (offset, table, column, extra) in enumerate(entries):
            end = (
                entries[index + 1][0]
                if index + 1 < len(entries)
                else block_size - header
            )
            sizes[(table, column)] += end - offset + extra * block_size
    connection.close()
    return dict(sizes), used * block_size


def sqlite_tables(path: Path) -> dict[str, int]:
    connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    rows = connection.execute(
        "SELECT name, SUM(pgsize) FROM dbstat GROUP BY name"
    ).fetchall()
    indexes = dict(
        connection.execute(
            "SELECT name, tbl_name FROM sqlite_master WHERE type = 'index'"
        ).fetchall()
    )
    connection.close()
    sizes = collections.Counter()
    for name, size in rows:
        sizes[indexes.get(name, name)] += size
    return dict(sizes)


def blob_bytes(roots: list[Path], bodies: set[str]) -> dict[str, int]:
    """Object bytes split into content bodies and extracted files by the registry that owns each hash."""
    sizes = collections.Counter()
    for root in roots:
        for path in root.rglob("*") if root.exists() else []:
            if path.is_file():
                sizes["content_bodies" if path.name in bodies else "files"] += (
                    path.stat().st_size
                )
    return dict(sizes)


def measure_embedded(work: Path) -> dict:
    columns, _used = duckdb_columns(work / "duckdb/sideseat.duckdb")
    # What the disk pays: the file, free blocks included (DuckDB reuses them but does not return them), and
    # any WAL not yet checkpointed.
    duck_total = sum(
        p.stat().st_size for p in (work / "duckdb").glob("sideseat.duckdb*")
    )
    sqlite_path = work / "sqlite/sideseat.db"
    connection = sqlite3.connect(f"file:{sqlite_path}?mode=ro", uri=True)
    bodies = {
        row[0] for row in connection.execute("SELECT body_hash FROM content_bodies")
    }
    connection.close()
    return {
        "analytics_columns": {f"{t}.{c}": b for (t, c), b in columns.items()},
        "analytics_total": duck_total,
        "transactional": sqlite_tables(sqlite_path),
        "blobs": blob_bytes([work / "files"], bodies),
    }


def measure_distributed() -> dict:
    tables = [
        t
        for t in clickhouse("SHOW TABLES FROM sideseat").split()
        if t in ANALYTICS_OWNER
    ]
    for table in tables:
        clickhouse(f"OPTIMIZE TABLE sideseat.{table} FINAL")
    rows = clickhouse(
        "SELECT table, name, data_compressed_bytes, data_uncompressed_bytes FROM system.columns "
        "WHERE database = 'sideseat' FORMAT TSV"
    )
    columns, uncompressed = {}, {}
    for line in rows.splitlines():
        table, name, compressed, raw = line.split("\t")
        if table in ANALYTICS_OWNER:
            columns[f"{table}.{name}"] = int(compressed)
            uncompressed[f"{table}.{name}"] = int(raw)
    parts = clickhouse(
        "SELECT table, sum(bytes_on_disk) FROM system.parts WHERE database = 'sideseat' AND active "
        "GROUP BY table FORMAT TSV"
    )
    on_disk = {t: int(b) for t, b in (line.split("\t") for line in parts.splitlines())}
    pg = docker(
        "exec",
        PG_NAME,
        "psql",
        "-U",
        "sideseat",
        "-d",
        "sideseat",
        "-At",
        "-F",
        "\t",
        "-c",
        "SELECT relname, pg_total_relation_size(relid) FROM pg_stat_user_tables",
    ).stdout
    transactional = {
        t: int(b) for t, b in (line.split("\t") for line in pg.splitlines() if line)
    }
    bodies = set(
        docker(
            "exec",
            PG_NAME,
            "psql",
            "-U",
            "sideseat",
            "-d",
            "sideseat",
            "-At",
            "-c",
            "SELECT body_hash FROM content_bodies",
        ).stdout.split()
    )
    listing = docker(
        "exec",
        MINIO_NAME,
        "sh",
        "-c",
        "find /data/sideseat-storage -name xl.meta -exec stat -c '%s %n' {} +",
    ).stdout
    blobs = collections.Counter()
    for line in listing.splitlines():
        size, name = line.split(" ", 1)
        blobs["content_bodies" if Path(name).parent.name in bodies else "files"] += int(
            size
        )
    return {
        "analytics_columns": columns,
        "analytics_uncompressed": uncompressed,
        "analytics_total": sum(on_disk.get(t, 0) for t in ANALYTICS_OWNER),
        "analytics_parts": on_disk,
        "transactional": transactional,
        "blobs": dict(blobs),
    }


def attribute(measured: dict) -> dict[str, collections.Counter]:
    """Stored bytes per signal and per layer."""
    layers = {s: collections.Counter() for s in (*SIGNALS, "fixed")}
    columns = measured["analytics_columns"]
    for key, size in columns.items():
        table = key.split(".", 1)[0]
        layers[ANALYTICS_OWNER.get(table, "fixed")][f"analytics:{table}"] += size
    # The exact total minus the attributed columns is per-file overhead (headers, free blocks, primary
    # indexes, marks). Spread over the signals in proportion to their column bytes.
    overhead = measured["analytics_total"] - sum(columns.values())
    signal_columns = {s: sum(v for k, v in layers[s].items()) for s in SIGNALS}
    total_columns = sum(signal_columns.values()) or 1
    for s in SIGNALS:
        layers[s]["analytics:overhead"] += round(
            overhead * signal_columns[s] / total_columns
        )
    for table, size in measured["transactional"].items():
        layers[TRANSACTIONAL_OWNER.get(table, "fixed")][f"transactional:{table}"] += (
            size
        )
    for kind, size in measured["blobs"].items():
        layers["traces"][f"blobs:{kind}"] += size
    return layers


def report(exports: list[dict], measured: dict, mode: str) -> dict:
    layers = attribute(measured)
    raw = collections.Counter()
    items = collections.Counter()
    for export in exports:
        raw[export["signal"]] += export["protobuf_bytes"]
        items[export["signal"]] += export["items"]
    result = {"mode": mode, "signals": {}}
    print(
        f"\n[storage] {mode}: whole corpus, {len({e['tenant'] for e in exports})} projects\n"
    )
    print(
        "| Signal | Exports | Items | Raw OTLP protobuf | Stored | Media | Stored excl. media | "
        "Raw B/item | Stored excl. media B/item |"
    )
    print("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |")
    for s in SIGNALS:
        stored = sum(layers[s].values())
        media = sum(layers[s].get(layer, 0) for layer in MEDIA_LAYERS)
        count = sum(1 for e in exports if e["signal"] == s)
        per_item = (stored - media) / items[s] if items[s] else None
        result["signals"][s] = {
            "exports": count,
            "items": items[s],
            "raw_bytes": raw[s],
            "stored_bytes": stored,
            "media_bytes": media,
            "stored_excluding_media_per_item": per_item,
            "layers": dict(layers[s].most_common()),
        }
        if not count:
            print(f"| {s} | 0 | 0 | - | {stored:,} | - | - | - | no corpus |")
            continue
        print(
            f"| {s} | {count} | {items[s]:,} | {raw[s]:,} | {stored:,} | {media:,} | "
            f"{stored - media:,} | {raw[s] / items[s]:,.0f} | {per_item:,.0f} |"
        )
    print(f"\nfixed (not attributed): {sum(layers['fixed'].values()):,} B")
    for s in SIGNALS:
        if not result["signals"][s]["exports"]:
            continue
        print(f"\n{s}: stored bytes by layer, per item")
        per = max(items[s], 1)
        for layer, size in layers[s].most_common():
            print(f"  {layer:44s} {size:>12,} B  {size / per:9.1f} B/item")
    print("\nlargest analytics columns")
    for key, size in sorted(
        measured["analytics_columns"].items(), key=lambda kv: -kv[1]
    )[:25]:
        print(f"  {key:52s} {size:>12,} B")
    result["measured"] = measured
    return result


def gate(result: dict) -> int:
    ceilings = json.loads(CEILING.read_text())[result["mode"]]
    failures = []
    for s, limits in ceilings.items():
        figure = result["signals"][s]["stored_excluding_media_per_item"]
        if not result["signals"][s]["exports"] or figure is None:
            failures.append(f"{s}: no corpus to measure")
            continue
        for kind in ("regression", "target"):
            if kind in limits and figure > limits[kind]:
                failures.append(
                    f"{s}: {figure:,.0f} B/item excluding media is above the {kind} ceiling of "
                    f"{limits[kind]:,} B/item"
                )
    for failure in failures:
        log(f"FAIL: {failure}")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("embedded", "distributed"))
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--json", type=Path)
    parser.add_argument("--gate", action="store_true")
    parser.add_argument(
        "--keep", action="store_true", help="keep the data directory for inspection"
    )
    args = parser.parse_args()
    target = Path(
        subprocess.run(
            ["bash", str(ROOT / "scripts/dev/cargo-target-dir.sh")],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    )
    binary = (args.binary or target / "release/sideseat").resolve()
    if not args.binary:
        log("building release")
        subprocess.run(
            ["cargo", "build", "--locked", "--release", "-q", "-p", "sideseat-server"],
            cwd=ROOT,
            check=True,
        )
    exports = corpus()
    log(f"corpus: {len(exports)} exports")
    work = Path(tempfile.mkdtemp(prefix="sideseat-storage-"))
    extra_env = {}
    server = None
    try:
        if args.mode == "distributed":
            extra_env = start_distributed(work)
        server = start_server(binary, work, extra_env)
        projects = load(exports)
        settle(projects)
        stop_server(server)
        server = None
        measured = (
            measure_embedded(work) if args.mode == "embedded" else measure_distributed()
        )
    finally:
        if server is not None:
            stop_server(server)
        if args.mode == "distributed":
            stop_distributed()
        if args.keep:
            log(f"data kept in {work}")
        else:
            shutil.rmtree(work, ignore_errors=True)
    result = report(exports, measured, args.mode)
    if args.json:
        args.json.write_text(json.dumps(result, indent=1, default=str))
    return gate(result) if args.gate else 0


if __name__ == "__main__":
    sys.exit(main())
