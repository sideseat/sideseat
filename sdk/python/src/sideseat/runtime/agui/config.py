# Adapted from observability-talk/demos/agent/agui/config.py
"""Configuration for the AG-UI renderer.

Decisions that used to live as `if t in (...)` in the renderer body are
hoisted here so users can override behavior without forking the renderer.
"""
from __future__ import annotations

from dataclasses import dataclass, field

from ag_ui.core import EventType


def _default_drop() -> set[EventType]:
    # Adapter-specific noise. Strict supersets of previous events that
    # carry no incremental information for human reading. Override if you
    # actually want to see them.
    return {EventType.MESSAGES_SNAPSHOT, EventType.STATE_DELTA}


def _default_dedup() -> set[EventType]:
    # Adapters often re-emit identical snapshots. Hash payload, skip if
    # unchanged. Note: dropped events never reach dedup.
    return {EventType.STATE_SNAPSHOT}


@dataclass
class RenderConfig:
    drop: set[EventType] = field(default_factory=_default_drop)
    dedup: set[EventType] = field(default_factory=_default_dedup)
    raw: bool = False  # bypass everything; print events as JSON

    @classmethod
    def show_all(cls) -> "RenderConfig":
        """Disable drops + dedup for inspection (still pretty-formatted)."""
        return cls(drop=set(), dedup=set())
