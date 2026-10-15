"""Which tables grow with the corpus, so none that holds a stored item's bytes is left out of every signal's figure.

A marginal measurement compares two replays. Every table, analytics or transactional, whose bytes grow between
them by more than `LIMIT` bytes per item of a signal holds something per item, and is charged to a signal by the
owner maps of `storage-footprint.py`. A table that grows and is in neither map fails the run unless it is listed
here with the reason its growth is no stored item: a per-record table left as "fixed" made every span figure, the
embedded verdict among them, omit the raw records' owner rows. Which signal a table belongs to is not inferred: a
replay grows every signal's items together, so the growth alone cannot tell a span's table from a log's.
"""

from __future__ import annotations

LIMIT = 0.5
# Tables that grow and hold no stored item, each with why.
NOT_STORED = {
    "staged_payloads": "payloads settled and deleted: the dead tuples' space, which the database reuses",
    "raw_writers": "write claims, each released when its record settles",
}


def growth(
    fewer: dict[str, int],
    more: dict[str, int],
    items: dict[str, int],
    owners: dict[str, str],
) -> tuple[list[str], list[str]]:
    """Every table's growth between the two replays, by bytes per item of each signal that grew, as report lines,
    and a failure for each that grows past `LIMIT` and is neither owned nor `NOT_STORED`."""
    lines, failures = [], []
    grown = {s: n for s, n in items.items() if n > 0}
    for table in sorted(set(fewer) | set(more)):
        added = more.get(table, 0) - fewer.get(table, 0)
        if not added:
            continue
        rates = {s: added / n for s, n in grown.items()}
        owner = owners.get(table) or (
            f"not stored: {NOT_STORED[table]}" if table in NOT_STORED else "unowned"
        )
        shown = ", ".join(f"{rate:.2f} B per {s} item" for s, rate in rates.items())
        lines.append(f"{table}: {added:+,} B, {shown} ({owner})")
        if owner == "unowned" and any(rate > LIMIT for rate in rates.values()):
            failures.append(
                f"{table} grows by {shown}, and no signal is charged for it: add it to an owner map in "
                f"storage-footprint.py, or to NOT_STORED in storage_growth.py with the reason"
            )
    return lines, failures
