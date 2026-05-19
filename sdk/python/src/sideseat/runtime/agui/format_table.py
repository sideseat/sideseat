# Adapted from observability-talk/demos/agent/agui/format_table.py
"""Per-EventType formatting spec.

Each row stores ``(style, inline_fn, payload_attr, block_renderer)``:
- *style*: rich style for the tag.
- *inline_fn*: ``key=val ...`` string for the header line.
- *payload_attr*: attribute on the event whose value is fed to the block
  renderer (``None`` means no block).
- *block_renderer*: how that payload is formatted as indented lines.

Adding a new event type = one row in ``TABLE``.
"""

from __future__ import annotations

from collections.abc import Callable

from ag_ui.core import BaseEvent, EventType
from rich.markup import escape as rich_escape

from .block_renderers import BlockRenderer, JsonBlock, TableBlock, TextBlock

InlineFn = Callable[[BaseEvent], str]
FormatSpec = tuple[str, InlineFn, str | None, BlockRenderer | None]

_EMPTY: InlineFn = lambda _e: ""  # noqa: E731

_JSON = JsonBlock()
_TEXT = TextBlock()
_TABLE_OR_JSON = TableBlock()  # auto-falls-back to JSON


SECTION_BEFORE = frozenset({EventType.STEP_STARTED, EventType.TOOL_CALL_START})
SECTION_AFTER = frozenset(
    {
        EventType.STEP_FINISHED,
        EventType.TOOL_CALL_RESULT,
        EventType.RUN_FINISHED,
        EventType.RUN_ERROR,
    }
)


def _attr(event: BaseEvent, name: str) -> str | None:
    value = getattr(event, name, None)
    return None if value is None else str(value)


def _kv(**fields: str | None) -> str:
    parts: list[str] = []
    for key, value in fields.items():
        if not value:
            continue
        rendered = repr(value) if (any(c.isspace() for c in value) or "=" in value) else value
        parts.append(f"{key}={rich_escape(rendered)}")
    return " ".join(parts)


# --- inline formatters ----------------------------------------------------


def _inline_ids(e: BaseEvent) -> str:
    return _kv(thread=_attr(e, "thread_id"), run=_attr(e, "run_id"))


def _inline_step(e: BaseEvent) -> str:
    return _kv(name=_attr(e, "step_name"))


def _inline_msg_role(e: BaseEvent) -> str:
    return _kv(id=_attr(e, "message_id"), role=_attr(e, "role"))


def _inline_msg_id(e: BaseEvent) -> str:
    return _kv(id=_attr(e, "message_id"))


def _inline_error(e: BaseEvent) -> str:
    return _kv(code=_attr(e, "code"))


def _inline_tool_start(e: BaseEvent) -> str:
    return _kv(name=_attr(e, "tool_call_name"), id=_attr(e, "tool_call_id"))


def _inline_tool_result(e: BaseEvent) -> str:
    return _kv(id=_attr(e, "tool_call_id"), msg_id=_attr(e, "message_id"))


def _inline_custom(e: BaseEvent) -> str:
    return _kv(name=_attr(e, "name"))


TABLE: dict[EventType, FormatSpec] = {
    # Lifecycle
    EventType.RUN_STARTED: ("bold blue", _inline_ids, None, None),
    EventType.RUN_FINISHED: ("bold green", _inline_ids, None, None),
    EventType.RUN_ERROR: ("bold red", _inline_error, "message", _TEXT),
    EventType.STEP_STARTED: ("bold blue", _inline_step, None, None),
    EventType.STEP_FINISHED: ("bold blue", _inline_step, None, None),
    # Text envelopes (content streamed separately by renderer)
    EventType.TEXT_MESSAGE_START: ("cyan", _inline_msg_role, None, None),
    EventType.TEXT_MESSAGE_END: ("cyan", _inline_msg_id, None, None),
    # Tools
    EventType.TOOL_CALL_START: ("bold yellow", _inline_tool_start, None, None),
    EventType.TOOL_CALL_RESULT: ("bold yellow", _inline_tool_result, "content", _TABLE_OR_JSON),
    # State
    EventType.STATE_SNAPSHOT: ("dim", _EMPTY, "snapshot", _JSON),
    EventType.STATE_DELTA: ("dim", _EMPTY, "delta", _JSON),
    EventType.MESSAGES_SNAPSHOT: ("dim", _EMPTY, "messages", _JSON),
    # Escape hatches
    EventType.RAW: ("dim", _EMPTY, "event", _JSON),
    EventType.CUSTOM: ("dim", _inline_custom, "value", _JSON),
}


def format_event(event: BaseEvent) -> tuple[str, str, list[str], str]:
    """Return ``(tag, inline, block_lines, style)`` for *event*."""
    t = event.type
    tag = t.value if hasattr(t, "value") else str(t)
    spec = TABLE.get(t)
    if spec is None:
        # Unknown event — show full payload so nothing is silently lost.
        return tag, "", _JSON.render(getattr(event, "__dict__", {})), "dim"
    style, inline_fn, payload_attr, block = spec
    block_lines: list[str] = []
    if payload_attr and block is not None:
        block_lines = block.render(getattr(event, payload_attr, None))
    return tag, inline_fn(event), block_lines, style
