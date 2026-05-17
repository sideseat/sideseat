# Adapted from observability-talk/demos/agent/agui/state.py
"""Per-run rendering state."""
from __future__ import annotations

from dataclasses import dataclass, field

from .snapshot_dedup import SnapshotDedup
from .stream_buffer import StreamBuffer


@dataclass
class RenderState:
    base_label: str | None
    current_label: str | None
    streaming: str | None = None  # "text" | "reasoning" | None
    had_error: bool = False
    last_blank: bool = True
    tool_args: StreamBuffer = field(default_factory=StreamBuffer)
    snapshots: SnapshotDedup = field(default_factory=SnapshotDedup)
