"""AG-UI invocation flow mixed into the runtime WebSocket client."""

from __future__ import annotations

import importlib.util
import logging
import threading
from contextlib import suppress
from typing import Any

from sideseat.runtime.client_support import (
    _as_jsonable,
    _Invocation,
    _json_dumps_compact,
    _mute_strands_callbacks,
    _Registration,
)
from sideseat.runtime.protocol import Envelope, ErrorCode, make_envelope

logger = logging.getLogger("sideseat.runtime.client")

# Keep chunks below the server's 4 MiB WebSocket frame ceiling after base64
# expansion. These values mirror the server-side AG-UI chunk constants.
_AGUI_CHUNK_THRESHOLD_BYTES = 2_500 * 1024
_AGUI_CHUNK_PAYLOAD_BYTES = _AGUI_CHUNK_THRESHOLD_BYTES


class _InvocationMixin:
    """Contract for invocation behavior supplied by ``RuntimeClient``."""

    _registry_lock: Any
    _registrations: dict[tuple[str, str], _Registration]
    _invoke_lock: Any
    _invocations: dict[str, _Invocation]
    _busy_agents: set[tuple[str, str]]

    def _send_envelope(self, env: Envelope) -> None:
        raise NotImplementedError

    # ------------------------------------------------------------------
    # AG-UI invoke flow (v2)
    # ------------------------------------------------------------------

    # Walk order matches the server-side `find_by_name` so a name resolves
    # to the same kind on both sides. `mcp` is intentionally excluded — it's
    # not invokable through AG-UI run-agent.
    _INVOKABLE_KINDS = ("agent", "graph", "swarm")

    def _resolve_invokable(self, name: str) -> tuple[str, _Registration] | None:
        """Look up an invokable registration by name. Returns (kind, reg)
        for the first match, or None. Used by both _handle_invoke and
        _handle_cancel so the two paths can't drift."""
        with self._registry_lock:
            for kind in self._INVOKABLE_KINDS:
                reg = self._registrations.get((kind, name))
                if reg is not None:
                    return kind, reg
        return None

    def _handle_invoke(
        self,
        *,
        request_id: str,
        agent_name: str,
        run_input: Any,
    ) -> None:
        """Server pushed an invocation. Dispatch in a worker thread."""
        if not request_id or not agent_name:
            logger.warning("agent.invoke missing request_id or agent_name; dropping")
            return

        # 1. Gate on the optional [agui] extra. Renderer + ag_ui types are
        #    needed; bail out cleanly if missing.
        if importlib.util.find_spec("ag_ui") is None:
            self._send_invoke_error(
                request_id,
                "agui_extra_missing",
                'install "sideseat[runtime]" to accept invocations',
            )
            return

        # 2. Resolve the live registration kind-agnostically (agent/graph/swarm).
        resolved = self._resolve_invokable(agent_name)
        if resolved is None:
            # Distinguish "exists but not invokable" (mcp) from "not found".
            with self._registry_lock:
                exists_as_mcp = ("mcp", agent_name) in self._registrations
            if exists_as_mcp:
                self._send_invoke_error(
                    request_id,
                    "unsupported_backend",
                    f"{agent_name!r} is registered as an mcp; not invokable via run-agent",
                )
            else:
                self._send_invoke_error(
                    request_id,
                    "registration_not_found",
                    f"no live registration named {agent_name!r}",
                )
            return
        kind, reg = resolved
        if reg.live_instance is None:
            self._send_invoke_error(
                request_id,
                "registration_not_found",
                f"registration {agent_name!r} has no live instance",
            )
            return

        # 3. Reject non-inproc runtimes (v2 only supports in-process).
        runtime = reg.manifest.runtime or {}
        runtime_kind = runtime.get("kind") if isinstance(runtime, dict) else None
        if runtime_kind not in (None, "inproc"):
            self._send_invoke_error(
                request_id,
                "unsupported_runtime",
                f"runtime kind {runtime_kind!r} not supported in v2",
            )
            return

        # 4. Validate RunAgentInput shape via pydantic. Validate once, pass
        #    the parsed model to the worker so we don't pay for it twice.
        try:
            from ag_ui.core import RunAgentInput

            run_in = RunAgentInput.model_validate(run_input)
        except Exception as exc:
            self._send_invoke_error(request_id, "bad_run_input", str(exc))
            return

        # 5. Reject concurrent invokes of the same (kind, name). Different
        #    (kind, name) pairs run concurrently — invoking a graph composite
        #    while one of its inner agents runs directly is allowed.
        slot = (kind, agent_name)
        with self._invoke_lock:
            if slot in self._busy_agents:
                self._send_invoke_error(
                    request_id, "agent_busy", "agent is already running an invocation"
                )
                return
            self._busy_agents.add(slot)

            worker = threading.Thread(
                target=self._invoke_worker_entry,
                name=f"sideseat-invoke-{agent_name}-{request_id[:8]}",
                args=(request_id, agent_name, kind, reg.live_instance, run_in),
                daemon=True,
            )
            self._invocations[request_id] = _Invocation(
                request_id=request_id,
                agent_name=agent_name,
                kind=kind,
                worker=worker,
            )
            worker.start()

    def _handle_cancel(self, *, request_id: str) -> None:
        """Server requested cancellation of a running invoke."""
        if not request_id:
            return
        with self._invoke_lock:
            inv = self._invocations.get(request_id)
            if inv is None:
                return
            inv.cancelled = True
            reg = self._registrations.get((inv.kind, inv.agent_name))
        if reg is not None and reg.live_instance is not None:
            cancel = getattr(reg.live_instance, "cancel", None)
            if callable(cancel):
                try:
                    cancel()
                except Exception:
                    logger.debug("agent.cancel() raised", exc_info=True)

    def _invoke_worker_entry(
        self,
        request_id: str,
        agent_name: str,
        kind: str,
        live_instance: Any,
        run_in: Any,  # validated ag_ui.core.RunAgentInput
    ) -> None:
        """Outer wrapper around `_run_invoke_async` so a panic before
        `asyncio.run()` ever reaches the converter still cleans up."""
        import asyncio

        logger.info(
            "invoke start kind=%s name=%s request_id=%s thread_id=%s run_id=%s",
            kind,
            agent_name,
            request_id,
            getattr(run_in, "thread_id", None),
            getattr(run_in, "run_id", None),
        )
        try:
            asyncio.run(self._run_invoke_async(request_id, agent_name, kind, live_instance, run_in))
        except BaseException as exc:
            logger.error("invoke worker crashed", exc_info=exc)
            with suppress(Exception):
                self._send_invoke_error(request_id, "internal", str(exc))
        finally:
            with self._invoke_lock:
                self._invocations.pop(request_id, None)
                self._busy_agents.discard((kind, agent_name))
            logger.info(
                "invoke end kind=%s name=%s request_id=%s",
                kind,
                agent_name,
                request_id,
            )

    async def _run_invoke_async(
        self,
        request_id: str,
        agent_name: str,
        kind: str,
        obj: Any,
        run_in: Any,  # already-validated `ag_ui.core.RunAgentInput`
    ) -> None:
        from ag_ui.core import RunErrorEvent

        from sideseat.runtime.agui import (
            AgUiRenderer,
            strands_multiagent_to_agui,
            strands_run_to_agui,
        )

        renderer = AgUiRenderer(label=f"{agent_name}#{request_id[:8]}")

        # Lifecycle invariant: the chosen converter is the single source of
        # `RUN_STARTED` and the terminal `RUN_FINISHED`/`RUN_ERROR`. The
        # agent path delegates to `ag_ui_strands.StrandsAgent.run` (which
        # emits both); the multiagent path emits them itself. This loop
        # forwards events verbatim — never prepends or appends lifecycle.

        if kind == "agent":
            stream = strands_run_to_agui(obj, run_in, name=agent_name)
        elif kind in ("graph", "swarm"):
            stream = strands_multiagent_to_agui(obj, run_in, name=agent_name)
        else:
            err = RunErrorEvent(
                message=f"unsupported backend kind {kind!r}",
                code="unsupported_backend",
            )
            with suppress(Exception):
                self._send_agui_event(request_id, err, renderer)
            self._send_invoke_error(
                request_id, "unsupported_backend", f"kind {kind!r} not invokable"
            )
            with suppress(Exception):
                renderer.finish()
            return

        # Mute Strands' default callback handlers across the composite so
        # their per-node prints don't interleave with our renderer.
        with _mute_strands_callbacks(obj):
            try:
                async for event in stream:
                    if self._is_cancelled(request_id):
                        cancel = getattr(obj, "cancel", None)
                        if callable(cancel):
                            with suppress(Exception):
                                cancel()
                        break
                    self._send_agui_event(request_id, event, renderer)

                if self._is_cancelled(request_id):
                    err = RunErrorEvent(
                        message="cancelled",
                        code=ErrorCode.CANCELLED.value,
                    )
                    self._send_agui_event(request_id, err, renderer)
                    self._send_invoke_error(request_id, "cancelled", "cancelled by server")
                else:
                    self._send_invoke_complete(request_id)
            except Exception as exc:
                err = RunErrorEvent(message=str(exc), code="internal")
                with suppress(Exception):
                    self._send_agui_event(request_id, err, renderer)
                self._send_invoke_error(request_id, "internal", str(exc))
            finally:
                with suppress(Exception):
                    renderer.finish()

    def _is_cancelled(self, request_id: str) -> bool:
        with self._invoke_lock:
            inv = self._invocations.get(request_id)
            return bool(inv and inv.cancelled)

    def _send_agui_event(self, request_id: str, event: Any, renderer: Any) -> None:
        try:
            payload = event.model_dump(mode="json", by_alias=True, exclude_none=True)
        except Exception:
            payload = _as_jsonable(event)
        with suppress(Exception):
            self._send_agui_event_payload(request_id, payload)
        with suppress(Exception):
            renderer.emit(event)

    def _send_agui_event_payload(self, request_id: str, payload: Any) -> None:
        """Send an AG-UI event payload to the server. Splits into
        `agent.event.chunk` frames if the serialised JSON exceeds
        :data:`_AGUI_CHUNK_THRESHOLD_BYTES` so we never trip the WS frame
        cap.

        Each chunk is sent through the regular `_send_envelope` path so
        the send-lock is acquired and released **per chunk**. That keeps
        heartbeat pongs and unrelated invocations on the same SDK
        responsive while a multi-megabyte event ships. The server
        reassembler keys partials by `(request_id, group_id)`, so any
        interleaving of small frames between chunks of the same group
        is reassembled correctly.
        """
        body = _json_dumps_compact(payload).encode("utf-8")
        if len(body) <= _AGUI_CHUNK_THRESHOLD_BYTES:
            self._send_envelope(
                make_envelope("agent.event", {"request_id": request_id, "event": payload})
            )
            return

        import base64
        import math
        import uuid as _uuid

        chunk_size = _AGUI_CHUNK_PAYLOAD_BYTES
        total = math.ceil(len(body) / chunk_size)
        group_id = str(_uuid.uuid4())
        for idx in range(total):
            slice_ = body[idx * chunk_size : (idx + 1) * chunk_size]
            self._send_envelope(
                make_envelope(
                    "agent.event.chunk",
                    {
                        "request_id": request_id,
                        "group_id": group_id,
                        "idx": idx,
                        "total": total,
                        "data_b64": base64.b64encode(slice_).decode("ascii"),
                    },
                )
            )

    def _send_invoke_complete(self, request_id: str) -> None:
        with suppress(Exception):
            self._send_envelope(make_envelope("agent.complete", {"request_id": request_id}))

    def _send_invoke_error(self, request_id: str, code: str, message: str) -> None:
        with suppress(Exception):
            self._send_envelope(
                make_envelope(
                    "agent.error",
                    {"request_id": request_id, "code": code, "message": message},
                )
            )
