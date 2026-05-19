# Adapted from observability-talk/demos/agent/agui/stream_buffer.py
"""Generic accumulator for AG-UI streaming `_CONTENT` / `_ARGS` / `_CHUNK` deltas.

AG-UI breaks large payloads (tool args, text, reasoning) into many small
delta events. Rendering each delta as its own line spams the console with
2-character JSON fragments. We buffer by key (typically tool_call_id or
message_id) and flush as one block on the matching `_END` event.
"""

from __future__ import annotations


class StreamBuffer:
    def __init__(self) -> None:
        self._buffers: dict[str, str] = {}

    def feed(self, key: str, delta: str | None) -> None:
        if not delta:
            return
        self._buffers[key] = self._buffers.get(key, "") + delta

    def flush(self, key: str) -> str:
        return self._buffers.pop(key, "")

    def has(self, key: str) -> bool:
        return key in self._buffers
