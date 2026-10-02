"""Shared data structures and helpers for the runtime WebSocket client."""

from __future__ import annotations

import os
import platform
import socket
from collections.abc import Iterator
from contextlib import contextmanager, suppress
from dataclasses import dataclass
from typing import Any

from sideseat._version import __version__
from sideseat.runtime.protocol import Envelope, RegistrationManifest


def _iter_strands_agents(obj: Any) -> list[Any]:
    """Return the Strands `Agent` instances reachable from `obj`.

    For a single Agent: just `[obj]`. For a Graph/Swarm: walks `obj.nodes`
    and yields each node's executor when it's an Agent. Recursive — nested
    composites surface their inner agents too. Used to mute every Agent's
    `callback_handler` for the duration of an invoke so Strands' default
    per-agent prints don't interleave with the AG-UI renderer.
    """
    out: list[Any] = []
    seen: set[int] = set()

    def visit(node: Any) -> None:
        if node is None or id(node) in seen:
            return
        seen.add(id(node))
        cls_name = type(node).__name__
        if cls_name == "Agent":
            out.append(node)
            return
        nodes = getattr(node, "nodes", None)
        if isinstance(nodes, dict):
            for n in nodes.values():
                visit(getattr(n, "executor", None))

    visit(obj)
    return out


@contextmanager
def _mute_strands_callbacks(obj: Any) -> Iterator[None]:
    """Context manager that swaps every reachable Strands Agent's
    `callback_handler` for `null_callback_handler` and restores the
    originals on exit. Quietly no-ops if Strands isn't importable
    (e.g. non-Strands agents)."""
    try:
        from strands.handlers.callback_handler import null_callback_handler
    except Exception:  # pragma: no cover — Strands is the only supported backend today
        yield
        return

    agents = _iter_strands_agents(obj)
    originals: list[tuple[Any, Any]] = []
    try:
        for ag in agents:
            originals.append((ag, getattr(ag, "callback_handler", None)))
            with suppress(Exception):
                ag.callback_handler = null_callback_handler
        yield
    finally:
        for ag, original in originals:
            with suppress(Exception):
                ag.callback_handler = original


@dataclass
class _Registration:
    kind: str  # "agent" | "mcp" | "swarm" | "graph"
    name: str
    manifest: RegistrationManifest
    # Live runtime instance (e.g. strands.Agent). Held strongly so the
    # invoke handler can call `stream_async()` on it. None for `mcp` /
    # registrations that don't need invoke support.
    live_instance: Any | None = None


@dataclass
class _Invocation:
    request_id: str
    agent_name: str
    kind: str  # "agent" | "graph" | "swarm"
    worker: Any  # threading.Thread; type-quoted to avoid forward-ref clutter
    cancelled: bool = False


def _capture_caller_var_names(arg: Any) -> dict[int, str]:
    """Walk up the call stack to find local variable names that point at any
    of the objects we're about to register.

    Returns a dict keyed by ``id(obj)`` so callers can look up the variable
    name under which an object was passed to ``register()``. Empty dict when
    the caller frame is not inspectable (e.g. C extensions, eval).
    """
    import sys

    try:
        # Skip our own frame and the caller of ``_capture_caller_var_names``.
        outer = sys._getframe(2)
    except (AttributeError, ValueError):
        return {}

    targets: list[Any] = list(arg) if isinstance(arg, (list, tuple)) else [arg]
    target_ids = {id(t) for t in targets}
    found: dict[int, str] = {}

    # Names that aliased the targets inside our own machinery — they always
    # point at the user's object via the caller's stack frame, but they're
    # generic SDK parameter names that would mislead the UI ("objects"
    # comes from `register(self, objects, ...)`). Skip them so the search
    # falls through to the genuine user-side variable name.
    _SKIP_VAR_NAMES = {
        "self",
        "obj",
        "objects",
        "arg",
        "args",
        "instance",
        "fallback_name",
        "var_names",
    }

    from types import FrameType

    frame: FrameType | None = outer
    seen_frames = 0
    while frame is not None and seen_frames < 20 and target_ids:
        for var_name, var_val in frame.f_locals.items():
            if var_name in _SKIP_VAR_NAMES:
                continue
            vid = id(var_val)
            if vid in target_ids and vid not in found:
                found[vid] = var_name
        target_ids -= set(found.keys())
        frame = frame.f_back
        seen_frames += 1
    return found


def _json_dumps_compact(payload: Any) -> str:
    """Serialise a JSON-able payload with separator-compact form so the
    encoded byte length matches what `make_envelope().to_json()` will end
    up sending. We measure here to decide whether to chunk."""
    import json

    return json.dumps(payload, separators=(",", ":"))


def _as_jsonable(value: Any) -> Any:
    if hasattr(value, "model_dump"):
        try:
            return value.model_dump(mode="json", by_alias=True, exclude_none=True)
        except Exception:
            pass
    if isinstance(value, (str, int, float, bool, type(None))):
        return value
    if isinstance(value, dict):
        return {k: _as_jsonable(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_as_jsonable(v) for v in value]
    return repr(value)


def _payload_field(env: Envelope, key: str) -> Any:
    if isinstance(env.payload, dict):
        return env.payload.get(key)
    return None


def _merge_default_metadata(extra: dict[str, Any] | None) -> dict[str, Any]:
    base = {
        "sdk_version": __version__,
        "pid": os.getpid(),
        "hostname": socket.gethostname(),
        "python_version": platform.python_version(),
    }
    if extra:
        base.update(extra)
    return base
