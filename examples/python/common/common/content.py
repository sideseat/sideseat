"""Helpers for reading provider response content."""

from __future__ import annotations

from typing import Any


def first_text_block(response: Any) -> str:
    """Return the first text block of an Anthropic-style response.

    `response.content[0]` is not necessarily the text: reasoning-capable models put a
    thinking block first, and tool-using turns put a tool_use block there. Indexing
    position 0 raises AttributeError as soon as the model is swapped for one that thinks.
    """
    for block in getattr(response, "content", None) or []:
        if getattr(block, "type", None) == "text":
            return block.text
    return ""
