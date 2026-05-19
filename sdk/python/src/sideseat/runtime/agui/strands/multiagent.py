"""Strands Graph/Swarm → AG-UI bridge.

Adapts the dict-based event flow from
``strands.multiagent.{Graph,Swarm}.stream_async()`` into AG-UI events.
Each `multiagent_node_*` event becomes per-node ``STEP_STARTED`` /
``STEP_FINISHED`` brackets, with inner content (text, reasoning, tool
calls, tool results) translated by per-node :class:`StrandsEventTranslator`
instances so node boundaries don't bleed.

Lifecycle: this converter emits its own ``RunStartedEvent`` upfront and a
terminal ``RunFinishedEvent`` (or ``RunErrorEvent`` on failure). It is the
single source of run-lifecycle events for the multiagent path, mirroring
how ``ag_ui_strands.StrandsAgent.run`` owns lifecycle on the single-agent
path.
"""

from __future__ import annotations

import logging
from collections.abc import AsyncIterator
from typing import Any

from ag_ui.core import (
    BaseEvent,
    EventType,
    RunAgentInput,
    RunErrorEvent,
    RunFinishedEvent,
    RunStartedEvent,
    StepFinishedEvent,
    StepStartedEvent,
)

from sideseat.runtime.agui.strands.translator import StrandsEventTranslator

_log = logging.getLogger("sideseat.runtime.agui.strands.multiagent")


async def strands_multiagent_to_agui(
    obj: Any,
    run_input: RunAgentInput,
    *,
    name: str = "graph",
) -> AsyncIterator[BaseEvent]:
    """Drive a Strands Graph/Swarm and yield AG-UI events.

    Args:
        obj: Strands ``Graph`` or ``Swarm`` instance with a
            ``stream_async(prompt) -> AsyncIterator[dict]`` method.
        run_input: Validated ``RunAgentInput``; the last user message
            becomes the prompt.
        name: Display label for diagnostics.

    Yields:
        AG-UI ``BaseEvent`` instances. Always emits exactly one
        ``RunStartedEvent`` at the top and exactly one terminal
        ``RunFinishedEvent`` or ``RunErrorEvent`` at the end.
    """
    prompt = _last_user_text(run_input)
    if not prompt:
        # Caller handles the error envelope; we surface a single AG-UI
        # error frame so downstream renderers paint a clean error.
        yield RunStartedEvent(
            type=EventType.RUN_STARTED,
            thread_id=run_input.thread_id,
            run_id=run_input.run_id,
        )
        yield RunErrorEvent(
            type=EventType.RUN_ERROR,
            message="run_input has no user message text",
            code="bad_run_input",
        )
        return

    stream_async = getattr(obj, "stream_async", None)
    if not callable(stream_async):
        yield RunStartedEvent(
            type=EventType.RUN_STARTED,
            thread_id=run_input.thread_id,
            run_id=run_input.run_id,
        )
        yield RunErrorEvent(
            type=EventType.RUN_ERROR,
            message=(
                f"{type(obj).__name__} has no stream_async(prompt); "
                "multiagent converter requires it"
            ),
            code="unsupported_backend",
        )
        return

    yield RunStartedEvent(
        type=EventType.RUN_STARTED,
        thread_id=run_input.thread_id,
        run_id=run_input.run_id,
    )

    translators: dict[str, StrandsEventTranslator] = {}
    try:
        async for raw in stream_async(prompt):
            for ag_event in _translate(raw, translators):
                yield ag_event
        # Drain any translator left open by the runtime (defensive —
        # `multiagent_node_stop` should have flushed each one).
        for node_id, tr in list(translators.items()):
            for ag_event in tr.finish():
                yield ag_event
            translators.pop(node_id, None)
        yield RunFinishedEvent(
            type=EventType.RUN_FINISHED,
            thread_id=run_input.thread_id,
            run_id=run_input.run_id,
        )
    except Exception as exc:  # noqa: BLE001 — terminal error frame
        _log.warning("multiagent stream raised", exc_info=exc)
        # Best-effort: drain open translators before the error so any
        # buffered text/tool_calls aren't lost from the renderer.
        for node_id, tr in list(translators.items()):
            try:
                for ag_event in tr.finish():
                    yield ag_event
            except Exception:
                pass
            translators.pop(node_id, None)
        yield RunErrorEvent(
            type=EventType.RUN_ERROR,
            message=str(exc),
            code="multiagent_error",
        )


def _last_user_text(run_input: RunAgentInput) -> str | None:
    """Extract the most recent user-message text from a `RunAgentInput`."""
    messages = getattr(run_input, "messages", None) or []
    for msg in reversed(messages):
        role = getattr(msg, "role", None)
        if role != "user":
            continue
        content = getattr(msg, "content", None)
        if isinstance(content, str) and content:
            return content
        # AG-UI also allows structured content lists; extract text parts.
        if isinstance(content, list):
            parts: list[str] = []
            for item in content:
                text = item.get("text") if isinstance(item, dict) else getattr(item, "text", None)
                if isinstance(text, str):
                    parts.append(text)
            if parts:
                return "".join(parts)
    return None


def _translate(
    event: dict[str, Any],
    translators: dict[str, StrandsEventTranslator],
) -> list[BaseEvent]:
    """Map one Strands `Graph.stream_async` dict to AG-UI events."""
    kind = event.get("type")

    if kind == "multiagent_node_start":
        node_id = str(event.get("node_id") or "")
        if not node_id:
            return []
        if node_id not in translators:
            translators[node_id] = StrandsEventTranslator()
        return [StepStartedEvent(type=EventType.STEP_STARTED, step_name=node_id)]

    if kind == "multiagent_node_stream":
        node_id = str(event.get("node_id") or "")
        inner = event.get("event")
        if not isinstance(inner, dict):
            return []
        tr = translators.get(node_id)
        if tr is None:
            # `_stream` arrived before `_start` — defensive: spin one up
            # so we don't lose events on ordering surprises.
            tr = StrandsEventTranslator()
            translators[node_id] = tr
        return list(tr.translate(inner))

    if kind == "multiagent_node_stop":
        node_id = str(event.get("node_id") or "")
        out: list[BaseEvent] = []
        tr = translators.pop(node_id, None)
        if tr is not None:
            out.extend(tr.finish())
        if node_id:
            out.append(StepFinishedEvent(type=EventType.STEP_FINISHED, step_name=node_id))
        return out

    # multiagent_handoff, result, unknown shapes — silently skip; logging
    # at debug level avoids spamming production output.
    if kind:
        _log.debug("multiagent unhandled event kind=%r", kind)
    return []
