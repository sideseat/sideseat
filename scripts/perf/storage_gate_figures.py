"""The deterministic storage gate's measurement: each table's bytes, its indexes', and the exact ratchet.

Imported by `storage-footprint.py` (`--store`, `--same-layout`), which measures the stores
`server/tests/storage_gate.rs` leaves. Kept in its own module because the measurement script is long enough.
"""

from __future__ import annotations

import argparse
import collections
import json
import shutil
import tempfile
from pathlib import Path

# The engine settings every connection that writes to a gate store uses. DuckDB lays a checkpoint's segments out
# in the order its threads finish them, so a store checkpointed with several threads can differ between two runs
# of the same input; with one it cannot. `tests/storage_gate.rs` sets the same on the server's connection.
DUCKDB_CONFIG = {"threads": 1}


def log(message: str) -> None:
    print(f"[storage] {message}", flush=True)


def column_layouts(connection) -> dict:
    """Every persistent segment of every table: row group, column, rows, compression, block and offset."""
    return {
        table: connection.execute(
            f"SELECT row_group_id, column_path, segment_id, segment_type, start, count, compression, "
            f"block_id, block_offset, additional_block_ids FROM pragma_storage_info('{table}') "
            f"WHERE persistent ORDER BY ALL"
        ).fetchall()
        for (table,) in connection.execute(
            "SELECT table_name FROM duckdb_tables() ORDER BY table_name"
        ).fetchall()
    }


def index_bytes(path: Path) -> dict[str, int]:
    """Bytes each table's indexes take, by table: on a copy, each index dropped in turn and the used blocks counted.

    DuckDB reports no index's size, so this is what dropping it gives back - whole blocks, the unit the file is
    charged in. It holds only while the drop frees nothing else: with a table's last index gone, the checkpoint
    may vacuum its deleted rows and rewrite its columns, and those blocks would be charged to the index. So every
    table's segments are compared across each drop, and a drop that moved any refuses the measurement. The copy
    is thrown away.
    """
    import duckdb

    with tempfile.TemporaryDirectory(dir=path.parent) as scratch:
        copy = Path(scratch) / path.name
        shutil.copyfile(path, copy)
        connection = duckdb.connect(str(copy), config=DUCKDB_CONFIG)
        block_size = connection.execute(
            "SELECT block_size FROM pragma_database_size()"
        ).fetchone()[0]
        indexes = connection.execute(
            "SELECT index_name, table_name FROM duckdb_indexes() ORDER BY table_name, index_name"
        ).fetchall()
        sizes = collections.Counter()
        for index, table in indexes:
            layout = column_layouts(connection)
            before = connection.execute(
                "SELECT used_blocks FROM pragma_database_size()"
            ).fetchone()[0]
            connection.execute(f'DROP INDEX "{index}"')
            connection.execute("FORCE CHECKPOINT")
            after = connection.execute(
                "SELECT used_blocks FROM pragma_database_size()"
            ).fetchone()[0]
            moved = [
                name
                for name, segments in column_layouts(connection).items()
                if layout.get(name) != segments
            ]
            if moved:
                raise SystemExit(
                    f"[storage] dropping {index} rewrote the columns of {', '.join(moved)}, so the blocks it "
                    "freed are not the index's alone; its size cannot be measured by dropping it"
                )
            sizes[table] += (before - after) * block_size
        connection.close()
    return dict(sizes)


def segment_layout(path: Path) -> dict:
    """Every table's segments and the used blocks: two stores with one layout hold the same bytes in the same
    places, whatever their headers say."""
    import duckdb

    connection = duckdb.connect(str(path), read_only=True)
    layout = {
        "used_blocks": connection.execute(
            "SELECT used_blocks FROM pragma_database_size()"
        ).fetchone()[0],
        **column_layouts(connection),
    }
    connection.close()
    return layout


def differences(now, then, path: str = "") -> list[tuple[str, object, object]]:
    """Every leaf where two nested figures differ, as (path, then, now)."""
    if isinstance(now, dict) and isinstance(then, dict):
        found = []
        for key in sorted(set(now) | set(then)):
            found += differences(
                now.get(key), then.get(key), f"{path}.{key}" if path else key
            )
        return found
    return [] if now == then else [(path, then, now)]


def same_stores(ours: Path, theirs: Path, figures, layout_too: bool = True) -> int:
    """Fail unless two stores have the same figures - each table's columns and indexes, the residual, every SQLite
    table and the media - as `figures(store)` measures them, and, with `layout_too`, one DuckDB segment layout."""
    layout = segment_layout(ours / "duckdb/sideseat.duckdb")
    other = segment_layout(theirs / "duckdb/sideseat.duckdb")
    differing = [
        key
        for key in sorted(set(layout) | set(other))
        if layout.get(key) != other.get(key)
    ]
    if differing and layout_too:
        log(f"FAIL: the stores' layouts differ in {', '.join(differing)}")
        return 1
    moved = differences(figures(ours), figures(theirs))
    for path, then, now in moved:
        log(f"  {path}: {then} -> {now}")
    if moved:
        placed = "one segment layout" if not differing else "segments placed apart"
        log(f"FAIL: the stores have {placed} but different figures")
        return 1
    placed = (
        f"segments placed differently in {', '.join(differing)}"
        if differing
        else "one segment layout"
    )
    log(
        f"the two stores have the same figures and {placed} "
        f"({len(layout) - 1} tables, {layout['used_blocks']} blocks)"
    )
    return 0


def shown(path: Path, root: Path) -> str:
    """`path` relative to the repository when it lies inside it, else as it is."""
    return str(path.relative_to(root)) if path.is_relative_to(root) else str(path)


def ratchet(figures: dict, baseline: Path, update: bool, root: Path) -> int:
    """Hold the figures to the committed baseline exactly: an increase fails, and so does a decrease the baseline
    has not taken in, so the baseline only ever moves in a commit that says why."""
    if update:
        baseline.write_text(json.dumps(figures, indent=1) + "\n")
        log(f"baseline written: {shown(baseline, root)}")
        return 0
    if not baseline.exists():
        log(
            f"FAIL: no baseline at {shown(baseline, root)}; write one with --update-baseline"
        )
        return 1
    moved = differences(figures, json.loads(baseline.read_text()))
    for path, then, now in moved:
        numbers = isinstance(now, (int, float)) and isinstance(then, (int, float))
        direction = ("up" if now > then else "down") if numbers else "changed"
        log(f"  {path}: {then} -> {now} ({direction})")
    if moved:
        log(
            "FAIL: the stored bytes moved from the baseline. A rise is a regression unless the commit explains it; "
            "either way the commit that moves them updates the baseline (`make storage-gate ARGS=--update`) and "
            "says why in its body."
        )
        return 1
    log("stored bytes match the baseline")
    return 0


def stored_figures(store: Path, measure) -> dict:
    """The bytes of the stores `tests/storage_gate.rs` left in `store`, by table, with no signal's items.

    A table is its segments' bytes and its indexes'. DuckDB says where a segment starts, not how long it is, so a
    segment runs to the next one in its block and the last to the block's end: a block's unused tail is charged
    to the segment before it, and where segments of two tables share a block, a change to one can move that tail
    into the other's figure - which shows, since every figure is held exactly. What lies in no segment - headers,
    metadata, free-list blocks - is the residual, held beside them and charged to no signal. Media is apart.
    `measure(store, config)` is the footprint script's embedded measurement.
    """
    measured = measure(store, DUCKDB_CONFIG)
    indexes = index_bytes(store / "duckdb/sideseat.duckdb")
    tables: dict[str, dict] = {}
    for key, size in measured["analytics_columns"].items():
        table = key.split(".", 1)[0]
        tables.setdefault(table, {"columns": 0, "indexes": 0})["columns"] += size
    for table, size in indexes.items():
        tables.setdefault(table, {"columns": 0, "indexes": 0})["indexes"] += size
    owned = sum(t["columns"] + t["indexes"] for t in tables.values())
    return {
        "tables": dict(sorted(tables.items())),
        "residual": measured["analytics_total"] - owned,
        "analytics_used": measured["analytics_total"],
        "transactional": dict(sorted(measured["transactional"].items())),
        "media": measured["blobs"].get("files", 0),
    }


def store_figures(
    figures: dict,
    exports: list[dict],
    passes: int,
    signals: tuple[str, ...],
    analytics_owner: dict[str, str],
    transactional_owner: dict[str, str],
) -> dict:
    """The gate's figures: `figures`, and each signal's bytes - its own tables' - per item of its corpus."""
    items = collections.Counter()
    for export in exports:
        items[export["signal"]] += export["items"] * passes
    per_signal = {}
    for s in signals:
        stored = sum(
            t["columns"] + t["indexes"]
            for name, t in figures["tables"].items()
            if analytics_owner.get(name) == s
        ) + sum(
            size
            for name, size in figures["transactional"].items()
            if transactional_owner.get(name) == s
        )
        per_signal[s] = {
            "items": items[s],
            "stored_excluding_media": stored,
            "per_item": round(stored / items[s], 2) if items[s] else None,
        }
    return {"passes": passes, **figures, "signals": per_signal}


def add_arguments(parser: argparse.ArgumentParser) -> None:
    """The storage gate's options: pinning its corpus, and measuring the stores its replay leaves."""
    parser.add_argument(
        "--pin-only",
        action="store_true",
        help="with --update-manifest: pin the corpus and stop, measuring nothing",
    )
    parser.add_argument(
        "--store",
        type=Path,
        help="measure the stores `tests/storage_gate.rs` left in this directory instead of running a server",
    )
    parser.add_argument(
        "--passes",
        type=int,
        default=1,
        help="how many passes of the corpus the --store replay made",
    )
    parser.add_argument(
        "--baseline",
        type=Path,
        help="hold the --store figures to this baseline exactly",
    )
    parser.add_argument(
        "--update-baseline",
        action="store_true",
        help="write the --store figures as the baseline",
    )
    parser.add_argument(
        "--same-layout",
        type=Path,
        help="with --store: fail unless this other store has the same DuckDB segment layout and figures",
    )
    parser.add_argument(
        "--same-figures",
        type=Path,
        help="with --store: fail unless this other store has the same figures, wherever its segments sit",
    )
    parser.add_argument(
        "--same-answers",
        type=Path,
        help="with --store: fail unless a server over this other store answers every read alike",
    )


def run(args: argparse.Namespace, script) -> int:
    """`--store`: measure the gate's stores, compare them with another, or hold them to `--baseline`. `script` is
    the footprint script, whose corpus, measurement and server this uses."""
    import storage_gate_answers

    store = args.store.resolve()
    if args.same_answers:
        return storage_gate_answers.same_answers(
            store, args.same_answers.resolve(), script, args.binary
        )
    other = args.same_layout or args.same_figures
    if other:
        return same_stores(
            store,
            other.resolve(),
            lambda path: stored_figures(path, script.measure_embedded),
            layout_too=bool(args.same_layout),
        )
    owners = (script.SIGNALS, script.ANALYTICS_OWNER, script.TRANSACTIONAL_OWNER)
    figures = store_figures(
        stored_figures(store, script.measure_embedded),
        script.corpus(),
        args.passes,
        *owners,
    )
    for s, signal in figures["signals"].items():
        log(
            f"{s}: {signal['stored_excluding_media']:,} B over {signal['items']:,} items, "
            f"{signal['per_item']} B/item excluding media"
        )
    log(
        f"residual {figures['residual']:,} B of {figures['analytics_used']:,} used; media {figures['media']:,} B"
    )
    if args.json:
        args.json.write_text(json.dumps(figures, indent=1))
    if args.baseline:
        return ratchet(
            figures, args.baseline.resolve(), args.update_baseline, script.ROOT
        )
    return 0
