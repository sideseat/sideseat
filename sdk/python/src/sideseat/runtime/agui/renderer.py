# Adapted from observability-talk/demos/agent/agui/renderer.py
"""AG-UI event stream → rich console.

Decoupled from Strands: takes any ``AsyncIterator[BaseEvent]`` and prints
it. Adapter-specific decisions (which events to drop, which to dedup) live
in ``RenderConfig`` so callers can override without touching this module.

Pipeline per event:
  1. raw mode → emit JSON line, return.
  2. streaming chunk (TEXT/REASONING content) → flow on the same line.
  3. TOOL_CALL_ARGS → buffer by tool_call_id.
  4. TOOL_CALL_END / _RESULT → flush buffered args under the START header.
  5. config.drop / config.dedup → skip if applicable.
  6. format via TABLE + block renderer → print.

`render_stream(events, ...)` is the consumer-facing function. The class
form is kept for cases (HITL) that need to interleave events with prompts.
"""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator

from ag_ui.core import BaseEvent, EventType
from rich.console import Console
from rich.json import JSON
from rich.markup import escape as rich_escape

from .block_renderers import JsonBlock
from .config import RenderConfig
from .format_table import SECTION_AFTER, SECTION_BEFORE, format_event
from .state import RenderState

_TOOL_ARGS_BLOCK = JsonBlock()


async def render_stream(
    events: AsyncIterator[BaseEvent],
    *,
    console: Console | None = None,
    label: str | None = None,
    config: RenderConfig | None = None,
) -> bool:
    """Drive a renderer to completion; return True iff RUN_ERROR was seen."""
    renderer = AgUiRenderer(console=console, label=label, config=config)
    try:
        async for event in events:
            renderer.emit(event)
    except (KeyboardInterrupt, asyncio.CancelledError):
        renderer.console.print("[yellow][INTERRUPTED][/yellow]")
    finally:
        renderer.finish()
    return renderer.had_error


class AgUiRenderer:
    def __init__(
        self,
        *,
        console: Console | None = None,
        label: str | None = None,
        config: RenderConfig | None = None,
    ) -> None:
        self.console = console or Console(soft_wrap=True, highlight=False)
        self.config = config or RenderConfig()
        self._state = RenderState(base_label=label, current_label=label)

    @property
    def had_error(self) -> bool:
        return self._state.had_error

    def set_label(self, label: str | None) -> None:
        self._state.base_label = label
        self._state.current_label = label

    def emit(self, event: BaseEvent) -> None:
        if self.config.raw:
            self._emit_raw(event)
        else:
            self._emit_pretty(event)

    def finish(self) -> None:
        if self._state.streaming:
            self.console.print()
            self._state.streaming = None

    # ---- raw mode ------------------------------------------------------

    def _emit_raw(self, event: BaseEvent) -> None:
        """The raw AG-UI event, pretty-printed as indented JSON. No
        buffering, no dedup, no filtering — exactly what the adapter
        emitted, in a form a human can read."""
        try:
            payload = event.model_dump_json()
        except AttributeError:
            payload = repr(getattr(event, "__dict__", {}))
        self.console.print(JSON(payload, indent=2))
        if event.type == EventType.RUN_ERROR:
            self._state.had_error = True

    # ---- pretty mode ---------------------------------------------------

    def _emit_pretty(self, event: BaseEvent) -> None:
        t = event.type
        state = self._state
        cfg = self.config

        # Streaming text — flow on the same line.
        if t == EventType.TEXT_MESSAGE_CONTENT:
            self.console.out(getattr(event, "delta", "") or "", end="", style="cyan")
            state.streaming = "text"
            return
        if t == EventType.REASONING_MESSAGE_CONTENT:
            self.console.out(getattr(event, "delta", "") or "", end="", style="dim italic")
            state.streaming = "reasoning"
            return

        # Buffer tool args; flush on TOOL_CALL_END or _RESULT.
        if t == EventType.TOOL_CALL_ARGS:
            tid = getattr(event, "tool_call_id", "") or ""
            state.tool_args.feed(tid, getattr(event, "delta", None))
            return

        # Close any open stream before structural events.
        if state.streaming is not None:
            self.console.print()
            state.streaming = None
            state.last_blank = False

        if t in (EventType.TOOL_CALL_END, EventType.TOOL_CALL_RESULT):
            tid = getattr(event, "tool_call_id", "") or ""
            if state.tool_args.has(tid):
                self._print_block(_TOOL_ARGS_BLOCK.render(state.tool_args.flush(tid)))
            if t == EventType.TOOL_CALL_END:
                return  # _END is silent in pretty mode

        # Config-driven filtering / dedup.
        if t in cfg.drop:
            return
        if t in cfg.dedup:
            payload = self._snapshot_payload(event)
            if not state.snapshots.is_new(t.value, payload):
                return

        # Side effects.
        if t == EventType.STEP_STARTED:
            step = getattr(event, "step_name", None)
            if step:
                state.current_label = step
        if t == EventType.RUN_ERROR:
            state.had_error = True

        # Visual rhythm.
        if t in SECTION_BEFORE and not state.last_blank:
            self.console.print()
            state.last_blank = True

        tag, inline, block, style = format_event(event)
        label_prefix = f"[dim]\\[{state.current_label}][/dim] " if state.current_label else ""
        line = f"{label_prefix}[{style}][{tag}][/{style}]"
        if inline:
            line = f"{line} [dim]{inline}[/dim]"
        self.console.print(line)
        state.last_blank = False
        self._print_block(block)

        if t == EventType.STEP_FINISHED:
            state.current_label = state.base_label
        if t in SECTION_AFTER:
            self.console.print()
            state.last_blank = True

    def _print_block(self, lines: list[str]) -> None:
        if not lines:
            return
        for body_line in lines:
            self.console.print(f"    [dim]│[/dim] {rich_escape(body_line)}")
        self._state.last_blank = False

    @staticmethod
    def _snapshot_payload(event: BaseEvent) -> object:
        for attr in ("snapshot", "messages", "delta", "value"):
            if hasattr(event, attr):
                return getattr(event, attr)
        return getattr(event, "__dict__", {})
