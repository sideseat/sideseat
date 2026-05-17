# Adapted from observability-talk/demos/agent/agui/block_renderers.py
"""Pluggable renderers for the indented "block" portion under each event.

A BlockRenderer takes a raw payload (str | dict | list) and turns it into
a list of pre-formatted lines (without the indent prefix — the caller adds
it). Choosing the right one lets a 50-row pipe-delimited tool result land
as a real table instead of grey indented text.
"""
from __future__ import annotations

import json
from typing import Any, Protocol


class BlockRenderer(Protocol):
    def render(self, payload: Any) -> list[str]: ...


class JsonBlock:
    """Default. Pretty-print as indented JSON; raw splitlines on parse fail."""

    def render(self, payload: Any) -> list[str]:
        if payload is None:
            return []
        if isinstance(payload, str):
            try:
                payload = json.loads(payload)
            except (ValueError, TypeError):
                return payload.splitlines() or [payload]
        try:
            return json.dumps(payload, ensure_ascii=False, indent=2).splitlines()
        except (TypeError, ValueError):
            return [repr(payload)]


class TextBlock:
    """No formatting — just splitlines. Use for free-form text payloads."""

    def render(self, payload: Any) -> list[str]:
        if payload is None:
            return []
        if isinstance(payload, str):
            return payload.splitlines() or [payload]
        return [repr(payload)]


class TableBlock:
    """Render a pipe-delimited table (`a | b\\n--- | ---\\nv1 | v2`) as
    aligned columns. Falls back to JsonBlock if the input doesn't look
    like a pipe-delimited table.

    Tool results from `run_sql` arrive double-encoded (JSON-encoded
    string of pipe-delimited text). We unwrap one JSON layer if present.
    """

    _FALLBACK = JsonBlock()

    def render(self, payload: Any) -> list[str]:
        text = self._coerce_to_text(payload)
        if text is None or "|" not in text:
            return self._FALLBACK.render(payload)

        rows = [r.strip() for r in text.splitlines() if r.strip()]
        if len(rows) < 2:
            return self._FALLBACK.render(payload)

        cells = [[c.strip() for c in row.split("|")] for row in rows]
        # Drop separator row if it looks like one (--- | --- | ...).
        if all(set(c) <= {"-"} for c in cells[1] if c):
            cells.pop(1)

        widths = [max(len(c) for c in col) for col in zip(*cells, strict=False)]
        out: list[str] = []
        for i, row in enumerate(cells):
            out.append("  ".join(c.ljust(w) for c, w in zip(row, widths, strict=False)))
            if i == 0:
                out.append("  ".join("─" * w for w in widths))
        return out

    @staticmethod
    def _coerce_to_text(payload: Any) -> str | None:
        if isinstance(payload, str):
            try:
                inner = json.loads(payload)
            except (ValueError, TypeError):
                return payload
            return inner if isinstance(inner, str) else None
        return None
