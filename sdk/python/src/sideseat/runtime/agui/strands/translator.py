"""Stateful translator: raw Strands stream-event dicts → AG-UI events.

Strands `Agent.stream_async()` emits a flow of dicts like:
- `{"data": "some text"}`                              — text delta
- `{"reasoningText": "...", "reasoning": True}`       — thinking delta
- `{"current_tool_use": {...}}`                       — tool-use streaming
- `{"event": {"contentBlockStop": {}}}`               — closes a block
- `{"message": {"role": "user", "content": [...]}}`   — tool_result
- `{"multiagent_node_start": ...}` / `_stop`          — multi-agent markers

We keep only the mapping surface we need in the graph / single-agent
paths: text, reasoning, tool_use, tool_result. Frontend-tool proxies
and state snapshots stay in `ag_ui_strands` where AG-UI clients need
them.

State is minimal:
- `message_id`        → id of the currently-streaming TEXT message
- `message_started`   → whether we've emitted TEXT_MESSAGE_START
- `reasoning_id` + `reasoning_started` — same for thinking
- `tool_calls_seen`   → tool_use_id → {name, args, input, emitted}

Ported verbatim from the aws-engagement reference implementation. Pure
dependency on `ag_ui.core` + stdlib — no SideSeat-specific context.
"""

from __future__ import annotations

import json
import uuid
from collections.abc import AsyncIterable, Iterable, Iterator
from typing import Any

from ag_ui.core import (
    BaseEvent,
    CustomEvent,
    EventType,
    ReasoningEndEvent,
    ReasoningMessageContentEvent,
    ReasoningMessageEndEvent,
    ReasoningMessageStartEvent,
    ReasoningStartEvent,
    TextMessageContentEvent,
    TextMessageEndEvent,
    TextMessageStartEvent,
    ToolCallArgsEvent,
    ToolCallEndEvent,
    ToolCallResultEvent,
    ToolCallStartEvent,
)


async def collect_text(events: AsyncIterable[BaseEvent]) -> str:
    """Run an AG-UI event stream to completion and return the
    concatenated assistant-text deltas."""
    collected = ""
    async for event in events:
        if isinstance(event, TextMessageContentEvent):
            collected += event.delta or ""
    return collected


class StrandsEventTranslator:
    """One translator instance per agent run (or per graph node).

    `translate(event)` takes a single Strands stream-event dict and
    yields zero or more AG-UI events. `finish()` closes any still-open
    text / reasoning message — call once when the source stream ends
    so the last message doesn't leak as "streaming" forever.
    """

    def __init__(self) -> None:
        self._message_id: str = _new_id()
        self._message_started: bool = False

        self._reasoning_id: str | None = None
        self._reasoning_started: bool = False

        self._tool_calls_seen: dict[str, dict[str, Any]] = {}

    # ---- public API --------------------------------------------------

    def translate(self, event: dict[str, Any]) -> Iterator[BaseEvent]:
        """Emit AG-UI events for one raw Strands stream event."""

        if event.get("init_event_loop") or event.get("start_event_loop"):
            return
        if event.get("complete") or event.get("force_stop"):
            return

        if "data" in event and event["data"]:
            if not self._message_started:
                yield TextMessageStartEvent(
                    type=EventType.TEXT_MESSAGE_START,
                    message_id=self._message_id,
                    role="assistant",
                )
                self._message_started = True
            yield TextMessageContentEvent(
                type=EventType.TEXT_MESSAGE_CONTENT,
                message_id=self._message_id,
                delta=str(event["data"]),
            )
            return

        if "reasoningText" in event and event.get("reasoning"):
            reasoning_text = event["reasoningText"]
            if not self._reasoning_started:
                self._reasoning_id = _new_id()
                yield ReasoningStartEvent(
                    type=EventType.REASONING_START,
                    message_id=self._reasoning_id,
                )
                yield ReasoningMessageStartEvent(
                    type=EventType.REASONING_MESSAGE_START,
                    message_id=self._reasoning_id,
                    role="reasoning",
                )
                self._reasoning_started = True
            assert self._reasoning_id is not None  # set above when starting
            if reasoning_text:
                yield ReasoningMessageContentEvent(
                    type=EventType.REASONING_MESSAGE_CONTENT,
                    message_id=self._reasoning_id,
                    delta=reasoning_text,
                )
            return

        if "current_tool_use" in event and event["current_tool_use"]:
            self._accumulate_tool_use(event["current_tool_use"])
            return

        inner_event = event.get("event")
        if isinstance(inner_event, dict) and "contentBlockStop" in inner_event:
            yield from self._close_reasoning_if_open()
            yield from self._emit_pending_tool_call()
            return

        message = event.get("message")
        if isinstance(message, dict) and message.get("role") == "user":
            yield from self._emit_tool_results(message)
            return

    def finish(self) -> Iterator[BaseEvent]:
        """Close any open streaming messages. Call once at stream end."""
        yield from self._close_reasoning_if_open()
        if self._message_started:
            yield TextMessageEndEvent(
                type=EventType.TEXT_MESSAGE_END,
                message_id=self._message_id,
            )
            self._message_started = False

    # ---- internals ---------------------------------------------------

    def _accumulate_tool_use(self, tool_use: dict[str, Any]) -> None:
        tool_name = tool_use.get("name")
        tool_use_id = tool_use.get("toolUseId")
        if not tool_name or not tool_use_id:
            return

        raw_input = tool_use.get("input", "")
        if isinstance(raw_input, str) and raw_input:
            try:
                parsed_input: Any = json.loads(raw_input)
            except json.JSONDecodeError:
                parsed_input = raw_input
        elif isinstance(raw_input, dict):
            parsed_input = raw_input
        else:
            parsed_input = {}

        args_str = json.dumps(parsed_input) if isinstance(parsed_input, dict) else str(parsed_input)

        entry = self._tool_calls_seen.get(tool_use_id)
        if entry is None:
            self._tool_calls_seen[tool_use_id] = {
                "name": tool_name,
                "input": parsed_input,
                "args": args_str,
                "emitted": False,
            }
        else:
            entry["input"] = parsed_input
            entry["args"] = args_str

    def _emit_pending_tool_call(self) -> Iterator[BaseEvent]:
        """On contentBlockStop, emit the most recent un-emitted tool
        call as a single START / ARGS / END triple."""
        target_id: str | None = None
        target: dict[str, Any] | None = None
        for tid, entry in self._tool_calls_seen.items():
            if not entry.get("emitted"):
                target_id = tid
                target = entry
                break
        if target is None or target_id is None:
            return
        target["emitted"] = True

        if self._message_started:
            yield TextMessageEndEvent(
                type=EventType.TEXT_MESSAGE_END,
                message_id=self._message_id,
            )
            self._message_started = False
            self._message_id = _new_id()

        yield ToolCallStartEvent(
            type=EventType.TOOL_CALL_START,
            tool_call_id=target_id,
            tool_call_name=str(target["name"]),
            parent_message_id=self._message_id,
        )
        yield ToolCallArgsEvent(
            type=EventType.TOOL_CALL_ARGS,
            tool_call_id=target_id,
            delta=str(target["args"]),
        )
        yield ToolCallEndEvent(
            type=EventType.TOOL_CALL_END,
            tool_call_id=target_id,
        )

    def _emit_tool_results(self, message: dict[str, Any]) -> Iterator[BaseEvent]:
        content = message.get("content", [])
        if not isinstance(content, list):
            return
        for item in content:
            if not isinstance(item, dict) or "toolResult" not in item:
                continue
            tool_result = item["toolResult"]
            tool_use_id = tool_result.get("toolUseId")
            if not tool_use_id:
                continue
            result_data = _extract_tool_result_text(tool_result.get("content"))
            if result_data is None:
                continue
            result_str = (
                json.dumps(result_data) if not isinstance(result_data, str) else result_data
            )
            yield from _emit_tool_result_fragmented(tool_use_id, result_str)

    def _close_reasoning_if_open(self) -> Iterator[BaseEvent]:
        if not self._reasoning_started or not self._reasoning_id:
            return
        rid = self._reasoning_id
        yield ReasoningMessageEndEvent(
            type=EventType.REASONING_MESSAGE_END,
            message_id=rid,
        )
        yield ReasoningEndEvent(
            type=EventType.REASONING_END,
            message_id=rid,
        )
        self._reasoning_started = False
        self._reasoning_id = None


def translate_events(events: Iterable[dict[str, Any]]) -> Iterator[BaseEvent]:
    """Convenience: drive one translator over a finite batch of events
    and flush at the end. Used by tests; production uses the class
    directly so it can translate one event at a time as they stream."""
    tr = StrandsEventTranslator()
    for raw in events:
        yield from tr.translate(raw)
    yield from tr.finish()


def _new_id() -> str:
    return uuid.uuid4().hex


# Tool-result fragmentation. AgentCore Runtime caps a single WebSocket
# frame at 64 KiB; large `TOOL_CALL_RESULT` payloads get chunked into
# `CUSTOM:TOOL_CALL_RESULT_CHUNK` events terminated by an empty
# `TOOL_CALL_RESULT`. Small results still go out as a single plain
# `TOOL_CALL_RESULT` so unaware clients keep working.
TOOL_RESULT_CHUNK_THRESHOLD_BYTES = 48 * 1024
TOOL_RESULT_CHUNK_SIZE_BYTES = 48 * 1024


def _emit_tool_result_fragmented(tool_use_id: str, content: str) -> Iterator[BaseEvent]:
    """Yield `TOOL_CALL_RESULT` or fragmented chunks + terminator."""
    encoded = content.encode("utf-8")
    if len(encoded) <= TOOL_RESULT_CHUNK_THRESHOLD_BYTES:
        yield ToolCallResultEvent(
            type=EventType.TOOL_CALL_RESULT,
            tool_call_id=tool_use_id,
            message_id=_new_id(),
            content=content,
        )
        return

    total = len(encoded)
    chunks: list[str] = []
    for offset in range(0, total, TOOL_RESULT_CHUNK_SIZE_BYTES):
        raw_piece = encoded[offset : offset + TOOL_RESULT_CHUNK_SIZE_BYTES]
        chunks.append(raw_piece.decode("utf-8", errors="ignore"))

    n_chunks = len(chunks)
    for idx, piece in enumerate(chunks):
        yield CustomEvent(
            type=EventType.CUSTOM,
            name="TOOL_CALL_RESULT_CHUNK",
            value={
                "tool_call_id": tool_use_id,
                "chunk_index": idx,
                "chunk_count": n_chunks,
                "content": piece,
            },
        )
    yield ToolCallResultEvent(
        type=EventType.TOOL_CALL_RESULT,
        tool_call_id=tool_use_id,
        message_id=_new_id(),
        content="",
    )


ToolResultPayload = dict[str, Any] | list[Any] | str | int | float | bool | None


def _extract_tool_result_text(content: Any) -> ToolResultPayload:
    """Find the text payload inside a toolResult block. Returns the
    parsed JSON object when possible, the raw string otherwise, or
    None if there's no text block."""
    if not isinstance(content, list):
        return None
    for item in content:
        if not isinstance(item, dict):
            continue
        text = item.get("text")
        if not isinstance(text, str):
            continue
        try:
            parsed: ToolResultPayload = json.loads(text)
            return parsed
        except json.JSONDecodeError:
            pass
        try:
            parsed = json.loads(text.replace("'", '"'))
            return parsed
        except (json.JSONDecodeError, ValueError):
            return text
    return None
