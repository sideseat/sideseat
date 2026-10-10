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
The corpus is the pinned set in `scripts/perf/storage-footprint-corpus.json`, so a fixture added by another
change cannot move the figure; `--update-manifest` pins the current fixtures deliberately. `--gate` fails the run when any signal exceeds its ceiling in `scripts/perf/storage-footprint-ceiling.json`: the
`target` the product promises, and a `regression` ceiling - the last measured figure plus a margin - so a change
that makes storage worse fails even while the target is still out of reach.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import storage_raw  # noqa: E402  (a sibling module, importable once the script's directory is on the path)
import storage_gate_figures  # noqa: E402  (the same)
import storage_server  # noqa: E402  (the same)
from storage_server import http  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / "server/tests/fixtures/messages"
# Metric exports have no message golden, so they live in their own corpus (`harness capture --metrics`).
METRIC_CORPUS = ROOT / "server/tests/fixtures/metrics"
CEILING = ROOT / "scripts/perf/storage-footprint-ceiling.json"
# The exact fixture set the gate measures, with each file's SHA-256. Pinned so a growing corpus cannot move the
# figure: a deliberate change rewrites this file (`--update-manifest`) and re-measures in the same commit.
MANIFEST = ROOT / "scripts/perf/storage-footprint-corpus.json"
# Tracked exports deliberately left out of the corpus, each with its reason. Every other tracked export is pinned,
# and the repository tests fail on one that is neither.
EXCLUDED = ROOT / "scripts/perf/storage-footprint-excluded.json"
# Metric points the derived load carries (`scripts/perf/metrics-load.json`), which the metric ceilings are
# stated on: 480 captured points cannot measure a store whose block is 256 KB.
METRIC_LOAD_POINTS = json.loads((ROOT / "scripts/perf/metrics-load.json").read_text())[
    "points"
]
# Layers that are media objects. Everything else, content-body copies included, is telemetry encoding.
MEDIA_LAYERS = ("blobs:files",)
SIGNALS = ("traces", "logs", "metrics")
# Raw records are the authority; only trace exports are stored as raw records so far.
RAW_TABLES = {"otel_raw": "traces"}
RAW_LAYERS = tuple(f"analytics:{table}" for table in RAW_TABLES)
SPAN_TABLES = {
    "otel_spans": "traces",
    "span_terms": "traces",
    "span_partition_anomalies": "traces",
    # Derived from the records: which hold spans of which trace, so a deletion can find them.
    "otel_raw_traces": "traces",
    "otel_raw_pending": "traces",
}
LOG_TABLES = {"otel_logs": "logs", "log_terms": "logs"}
METRIC_TABLES = {"otel_metrics": "metrics"}
ANALYTICS_OWNER = {**RAW_TABLES, **SPAN_TABLES, **LOG_TABLES, **METRIC_TABLES}
# Transactional tables whose rows exist per stored span or file. Everything else there is fixed.
TRANSACTIONAL_OWNER = {
    "files": "traces",
    "trace_files": "traces",
}
# Free ports of its own unless named: two runs on one port measured each other's corpus too.
PORT = int(os.environ.get("FOOTPRINT_STORAGE_PORT", "0")) or storage_server.free_ports()
SCOPE = str(abs(hash(str(ROOT))) % 1_000_000)
CH_NAME, PG_NAME, MINIO_NAME = (
    f"sideseat-storage-{n}-{SCOPE}" for n in ("ch", "pg", "minio")
)
CH_IMAGE = os.environ.get("CH_IMAGE_TAG", "26.4.3.37")


def log(message: str) -> None:
    print(f"[storage] {message}", flush=True)


# --- corpus ------------------------------------------------------------------------------------------


def tracked_files() -> set[Path]:
    """Every file git tracks under the fixture trees: the only inputs another checkout also has."""
    listed = subprocess.run(
        ["git", "ls-files", "-z", "--", str(CORPUS), str(METRIC_CORPUS)],
        cwd=ROOT,
        capture_output=True,
        check=True,
    ).stdout
    return {ROOT / name for name in listed.decode().split("\0") if name}


def corpus_paths() -> list[Path]:
    """Every tracked export file the fixture trees hold now, but those excluded with a reason. Local-only
    (ignored) captures are left out, so a figure measured here is one any checkout reproduces."""
    excluded = set(json.loads(EXCLUDED.read_text())) if EXCLUDED.exists() else set()
    paths = []
    for path in sorted(tracked_files()):
        if str(path.relative_to(ROOT)) in excluded:
            continue
        prefix = path.name.split("-", 1)[0]
        if (
            prefix in ("req", "logs", "metrics")
            and path.suffix in (".pb", ".json")
            and path.is_file()
        ):
            paths.append(path)
    return paths


def write_manifest() -> None:
    entries = {
        str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in corpus_paths()
    }
    MANIFEST.write_text(json.dumps(entries, indent=0, sort_keys=True) + "\n")
    log(f"manifest: {len(entries)} exports pinned in {MANIFEST.relative_to(ROOT)}")


def manifest_paths() -> list[Path]:
    """The pinned exports, refusing a run whose files changed since the manifest was written."""
    entries = json.loads(MANIFEST.read_text())
    tracked = tracked_files()
    paths, drift = [], []
    for relative, digest in sorted(entries.items()):
        path = ROOT / relative
        if path not in tracked:
            drift.append(f"untracked {relative}")
        elif not path.is_file():
            drift.append(f"missing {relative}")
        elif hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            drift.append(f"changed {relative}")
        else:
            paths.append(path)
    if drift:
        sys.exit(
            "[storage] the pinned corpus no longer matches the fixtures; pin it again deliberately "
            "with --update-manifest and re-measure:\n  " + "\n  ".join(drift[:20])
        )
    return paths


def corpus() -> list[dict]:
    """Every pinned export, with its signal, tenant and protobuf size."""
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
    for path in manifest_paths():
        prefix = path.name.split("-", 1)[0]
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


def derived_metrics(work: Path) -> list[dict]:
    """The deterministic metric load (`scripts/perf/metrics-load.py`), in place of the 480 captured points."""
    from opentelemetry.proto.collector.metrics.v1.metrics_service_pb2 import (
        ExportMetricsServiceRequest,
    )

    out = work / "metrics-load"
    subprocess.run(
        [
            "uv",
            "run",
            "--locked",
            "--script",
            str(ROOT / "scripts/perf/metrics-load.py"),
            str(out),
        ],
        check=True,
    )
    exports = []
    for path in sorted(out.rglob("metrics-*.pb")):
        body = path.read_bytes()
        message = ExportMetricsServiceRequest()
        message.ParseFromString(body)
        exports.append(
            {
                "path": path,
                "signal": "metrics",
                "tenant": path.parent.name,
                "body": body,
                "json": False,
                "protobuf_bytes": len(body),
                "items": count_items("metrics", message),
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


# --- measurement ---------------------------------------------------------------------------------------


def duckdb_settle(path: Path, config: dict | None = None) -> None:
    """Flush everything to blocks, so a measurement does not depend on when it was taken.

    `FORCE CHECKPOINT` writes the write-ahead log into the database file and rewrites partially-filled blocks;
    without it the same corpus measures differently depending on how much was still in the log. `VACUUM`
    afterwards is DuckDB's no-op for a freshly written file but keeps the sequence honest if that changes.
    """
    import duckdb

    connection = duckdb.connect(str(path), config=config or {})
    connection.execute("FORCE CHECKPOINT")
    connection.execute("VACUUM")
    connection.close()


def duckdb_blocks(path: Path) -> tuple[dict, int, int]:
    """Bytes per (table, column) from DuckDB's segment map, the used bytes, and the free bytes.

    DuckDB does not report a segment's size, only where it starts. A segment's extent is therefore the gap to
    the next segment in the same block (or to the block end), plus any overflow blocks it owns. The sum is the
    used blocks, so the per-column split is approximate while the total is exact.

    Free blocks are returned separately and never counted per item: they are capacity the file has already
    taken from the filesystem and will reuse, not bytes this corpus stores, and counting them made the figure
    depend on how much churn the ingest happened to leave behind.
    """
    import duckdb

    connection = duckdb.connect(str(path), read_only=True)
    block_size, total_blocks, used, free = connection.execute(
        "SELECT block_size, total_blocks, used_blocks, free_blocks FROM pragma_database_size()"
    ).fetchone()
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
    if total_blocks != used + free:
        log(f"warning: {total_blocks} blocks is not {used} used + {free} free")
    return dict(sizes), used * block_size, free * block_size


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


def blob_bytes(roots: list[Path]) -> dict[str, int]:
    """Object bytes. Every object is media now: the content-body store is retired."""
    sizes = collections.Counter()
    for root in roots:
        for path in root.rglob("*") if root.exists() else []:
            if path.is_file():
                sizes["files"] += path.stat().st_size
    return dict(sizes)


def duckdb_rows(path: Path) -> dict[str, int]:
    """Rows per analytics table."""
    import duckdb

    connection = duckdb.connect(str(path), read_only=True)
    tables = [
        row[0]
        for row in connection.execute(
            "SELECT table_name FROM duckdb_tables()"
        ).fetchall()
    ]
    rows = {
        table: connection.execute(f'SELECT count(*) FROM "{table}"').fetchone()[0]
        for table in tables
    }
    connection.close()
    return rows


def measure_embedded(work: Path, config: dict | None = None) -> dict:
    """Measure the embedded store after settling it, so the figure is a property of the corpus.

    Everything counted per item is a used block: the per-column attribution from the segment map plus the
    residue (indexes, headers, the unused tail of a last block), which is exactly `used_blocks`. Free blocks
    and any write-ahead log are reported beside it and excluded, since neither is bytes this corpus stores.
    """
    database = work / "duckdb/sideseat.duckdb"
    duckdb_settle(database, config)
    columns, used, free = duckdb_blocks(database)
    rows = duckdb_rows(database)
    wal = sum(
        p.stat().st_size
        for p in (work / "duckdb").glob("sideseat.duckdb*")
        if p.name != "sideseat.duckdb"
    )
    if wal:
        log(f"warning: {wal} bytes of write-ahead log survived the checkpoint")
    sqlite_path = work / "sqlite/sideseat.db"
    return {
        "analytics_columns": {f"{t}.{c}": b for (t, c), b in columns.items()},
        "analytics_total": used,
        "analytics_free": free,
        "analytics_wal": wal,
        "analytics_rows": rows,
        "transactional": sqlite_tables(sqlite_path),
        "blobs": blob_bytes([work / "files"]),
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
    listing = docker(
        "exec",
        MINIO_NAME,
        "sh",
        "-c",
        "find /data/sideseat-storage -name xl.meta -exec stat -c '%s %n' {} +",
    ).stdout
    blobs = collections.Counter()
    for line in listing.splitlines():
        size, _name = line.split(" ", 1)
        blobs["files"] += int(size)
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
    # The exact total minus the attributed columns is per-file overhead: headers, free blocks, ART indexes,
    # and the unused tail of each table's last block. ClickHouse reports it per table (a part's bytes minus its
    # columns'). DuckDB cannot, so it is spread over the signals by **rows**: indexes and churn scale with rows,
    # while spreading by column bytes would charge a small table more whenever a large one compressed better.
    overhead = measured["analytics_total"] - sum(columns.values())
    parts = measured.get("analytics_parts")
    if parts:
        for table, size in parts.items():
            if table in ANALYTICS_OWNER:
                table_columns = sum(
                    v for k, v in columns.items() if k.split(".", 1)[0] == table
                )
                layers[ANALYTICS_OWNER[table]]["analytics:overhead"] += max(
                    size - table_columns, 0
                )
    else:
        rows = measured.get("analytics_rows", {})
        signal_rows = {
            s: sum(n for t, n in rows.items() if ANALYTICS_OWNER.get(t) == s)
            for s in SIGNALS
        }
        total_rows = sum(signal_rows.values()) or 1
        for s in SIGNALS:
            layers[s]["analytics:overhead"] += round(
                overhead * signal_rows[s] / total_rows
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
    # Reported, never charged per item: capacity the file keeps for reuse, and a log the checkpoint should have
    # emptied. Counting either made the same corpus measure differently depending on the churn of the run.
    if "analytics_free" in measured:
        print(
            f"free blocks (reported, not charged): {measured['analytics_free']:,} B"
            + (
                f", write-ahead log {measured['analytics_wal']:,} B"
                if measured.get("analytics_wal")
                else ""
            )
        )
    for s in SIGNALS:
        if not result["signals"][s]["exports"]:
            continue
        print(f"\n{s}: stored bytes by layer, per item")
        per = max(items[s], 1)
        for layer, size in layers[s].most_common():
            print(f"  {layer:44s} {size:>12,} B  {size / per:9.1f} B/item")
        # Raw records are the authority; everything else excluding media is a cache rebuilt from them.
        raw_bytes = sum(v for k, v in layers[s].items() if k in RAW_LAYERS)
        media = sum(layers[s].get(layer, 0) for layer in MEDIA_LAYERS)
        derived = sum(layers[s].values()) - raw_bytes - media
        result["signals"][s]["raw_authority_per_item"] = raw_bytes / per
        result["signals"][s]["derived_cache_per_item"] = derived / per
        print(
            f"  raw (authority) {raw_bytes / per:,.1f} B/item, derived (cache) {derived / per:,.1f} B/item, "
            f"media {media / per:,.1f} B/item"
        )
    print("\nlargest analytics columns")
    for key, size in sorted(
        measured["analytics_columns"].items(), key=lambda kv: -kv[1]
    )[:25]:
        print(f"  {key:52s} {size:>12,} B")
    result["measured"] = measured
    return result


def gate(result: dict, only: str | None = None) -> int:
    """Fail the run when a signal is above its ceiling, or when the run is not the one the ceilings describe.

    Every signal is judged, on the whole corpus: `only` - a run of one signal's corpus - is refused rather than
    judged, since DuckDB's residue is spread over the signals by rows and a run without the others charges the
    one it holds a different share (see below).
    """
    ceilings = json.loads(CEILING.read_text())[result["mode"]]
    failures = []
    if only:
        failures.append(
            f"gating measures the whole corpus, every signal; --signal {only} loaded {only} alone, which "
            f"charges it all of the store's residue. Drop --signal, or drop --gate to measure one signal."
        )
    # The ceilings were taken on one invocation: the whole pinned corpus with the derived metric load. DuckDB
    # cannot attribute its residue - indexes, block tails, metadata - to a table, so the report spreads it over
    # the signals by rows: measuring a different set of rows charges every signal a different share, and the
    # figures are not the ones any ceiling was taken on. Measured on one corpus, the 480 captured points in place
    # of the load read 784 -> 867 B/span, 2,570 -> 2,717 B/log and 138 -> 552 B/point, which looks like three
    # regressions and is one wrong command. Such a run reports its figures and is refused rather than compared.
    metric_points = result["signals"]["metrics"]["items"]
    if metric_points < METRIC_LOAD_POINTS:
        failures.append(
            f"gating needs the whole corpus with the derived metric load, at least {METRIC_LOAD_POINTS:,} "
            f"points; this run measured {metric_points:,}, so each signal's share of the store's residue is "
            f"not the one the ceilings were taken on and no figure here is comparable with one. Run "
            f"`make footprint-storage`, or drop --gate to measure a subset."
        )
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
        "--metrics-load",
        action="store_true",
        help="measure metrics on the derived load of scripts/perf/metrics-load.json instead of the captures",
    )
    parser.add_argument(
        "--signal",
        choices=SIGNALS,
        help="load only this signal's corpus, so the figure is not affected by what else is stored",
    )
    parser.add_argument(
        "--verify-raw",
        action="store_true",
        help="decode every stored raw record and require the exact bytes each export was sent as",
    )
    parser.add_argument(
        "--update-manifest",
        action="store_true",
        help="pin the fixture set as it is now, then measure it",
    )
    parser.add_argument(
        "--keep", action="store_true", help="keep the data directory for inspection"
    )
    storage_gate_figures.add_arguments(parser)
    args = parser.parse_args()
    if args.update_manifest and args.pin_only:
        write_manifest()
        return 0
    if args.store:
        return storage_gate_figures.run(args, sys.modules[__name__])
    if args.update_manifest:
        write_manifest()
    binary = args.binary.resolve() if args.binary else None
    if binary is None:
        target = Path(
            subprocess.run(
                ["bash", str(ROOT / "scripts/dev/cargo-target-dir.sh")],
                capture_output=True,
                text=True,
                check=True,
            ).stdout.strip()
        )
        binary = (target / "release/sideseat").resolve()
        log("building release")
        subprocess.run(
            ["cargo", "build", "--locked", "--release", "-q", "-p", "sideseat-server"],
            cwd=ROOT,
            check=True,
        )
    exports = corpus()
    work = Path(tempfile.mkdtemp(prefix="sideseat-storage-"))
    if args.signal:
        exports = [e for e in exports if e["signal"] == args.signal]
    if args.metrics_load and args.signal in (None, "metrics"):
        exports = [e for e in exports if e["signal"] != "metrics"] + derived_metrics(
            work
        )
    log(f"corpus: {len(exports)} exports")
    extra_env = {}
    server = None
    # A failed run keeps its work directory, the server's log among it: a run that deletes the evidence of its
    # own failure leaves nothing to find the cause in.
    outcome = 1
    try:
        if args.mode == "distributed":
            extra_env = start_distributed(work)
        server = storage_server.start_server(binary, work, extra_env, PORT)
        projects = storage_server.load(exports, PORT, server, work, ROOT)
        storage_server.settle(projects, PORT, server, work)
        storage_server.stop_server(server)
        server = None
        measured = (
            measure_embedded(work) if args.mode == "embedded" else measure_distributed()
        )
        raw_failed = (
            storage_raw.verify_raw_embedded(work, exports, projects)
            if args.mode == "embedded" and args.verify_raw
            else 0
        )
        result = report(exports, measured, args.mode)
        if args.json:
            args.json.write_text(json.dumps(result, indent=1, default=str))
        outcome = max(gate(result, args.signal) if args.gate else 0, raw_failed)
        return outcome
    finally:
        if server is not None:
            storage_server.stop_server(server)
        if args.mode == "distributed":
            stop_distributed()
        if args.keep or outcome:
            log(f"data kept in {work}")
            if outcome:
                log(f"the server's last output:\n{storage_server.log_tail(work)}")
        else:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
