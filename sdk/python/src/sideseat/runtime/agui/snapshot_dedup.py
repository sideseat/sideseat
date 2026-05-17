# Adapted from observability-talk/demos/agent/agui/snapshot_dedup.py
"""Skip snapshots that haven't actually changed.

Some adapters (ag-ui-strands) emit MESSAGES_SNAPSHOT after every message,
each one a superset of the previous. Hashing the payload lets us drop
true duplicates while still showing genuine state transitions.
"""
from __future__ import annotations


class SnapshotDedup:
    def __init__(self) -> None:
        self._last: dict[str, int] = {}

    def is_new(self, kind: str, payload: object) -> bool:
        h = hash(repr(payload))
        if self._last.get(kind) == h:
            return False
        self._last[kind] = h
        return True
