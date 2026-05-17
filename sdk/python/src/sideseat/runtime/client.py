"""Persistent SideSeat WebSocket client.

Sync-only by design. One daemon I/O thread runs the receive loop; sends are
serialized via a single threading.Lock. Reconnect uses exponential backoff
with jitter and re-flushes the local registry on every reconnect.
"""

from __future__ import annotations

import atexit
import logging
import os
import platform
import random
import signal
import socket
import threading
import time
import uuid
from contextlib import suppress
from dataclasses import dataclass
from typing import Any
from urllib.parse import urlparse, urlunparse

from sideseat._utils import _module_available
from sideseat._version import __version__
from sideseat.runtime.adapters import (
    build_agent_manifest,
    build_manifest_for_kind,
    build_mcp_manifest,
    classify,
    derive_default_name,
)
from sideseat.runtime.protocol import (
    Envelope,
    ErrorCode,
    PROTOCOL_VERSION,
    RegistrationManifest,
    make_envelope,
    parse_envelope,
)

logger = logging.getLogger("sideseat.runtime.client")

_DEFAULT_HEARTBEAT_INTERVAL = 20
_DEFAULT_PONG_GRACE = 10
_RECONNECT_INITIAL = 0.25
_RECONNECT_MAX = 5.0
_RECONNECT_FAILURES_LOG_THRESHOLD = 30
_DEFAULT_MAX_MESSAGE_BYTES = 4 * 1024 * 1024


@dataclass(frozen=True)
class _Registration:
    kind: str  # "agent" | "mcp" | "swarm" | "graph"
    name: str
    manifest: RegistrationManifest


class RuntimeClient:
    """Persistent WS client. Sync API; daemon thread does the I/O."""

    def __init__(self, *, endpoint: str, project_id: str) -> None:
        self._endpoint = endpoint
        self._project_id = project_id

        # Use RLock so the I/O thread can call disconnect()→send_envelope()
        # which re-acquires the same lock (e.g. on a `replaced` notice).
        self._registry_lock = threading.RLock()
        self._registrations: dict[tuple[str, str], _Registration] = {}

        self._send_lock = threading.RLock()
        self._ws: Any | None = None  # websockets.sync.client.ClientConnection

        self._client_id = str(uuid.uuid4())
        self._connected_event = threading.Event()
        self._stop_event = threading.Event()
        self._stopped = threading.Event()
        self._thread: threading.Thread | None = None
        self._atexit_registered = False
        self._signal_handlers_installed = False

        self._last_server_message_at = 0.0
        self._heartbeat_interval = _DEFAULT_HEARTBEAT_INTERVAL
        self._banner_enabled = False
        self._banner_printed = False

    # ------------------------------------------------------------------
    # Public registration API
    # ------------------------------------------------------------------

    def add_agent(
        self,
        instance: Any,
        *,
        name: str,
        runtime: str | dict[str, Any] = "inproc",
        agentcore_endpoint: str | None = None,
        tools: list[Any] | None = None,
        system_prompt: str | None = None,
        model: str | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> None:
        manifest = build_agent_manifest(
            instance,
            name=name,
            runtime=runtime,
            agentcore_endpoint=agentcore_endpoint,
            tools=tools,
            system_prompt=system_prompt,
            model=model,
            metadata=_merge_default_metadata(metadata),
        )
        self._upsert_registration(_Registration(kind="agent", name=name, manifest=manifest))

    def add_mcp(
        self,
        client: Any,
        *,
        name: str,
        transport: str | None = None,
        url: str | None = None,
        tools: list[Any] | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> None:
        manifest = build_mcp_manifest(
            client,
            name=name,
            transport=transport,
            url=url,
            tools=tools,
            metadata=_merge_default_metadata(metadata),
        )
        self._upsert_registration(_Registration(kind="mcp", name=name, manifest=manifest))

    def remove_agent(self, name: str) -> None:
        self._remove_registration("agent", name)

    def remove_mcp(self, name: str) -> None:
        self._remove_registration("mcp", name)

    # ------------------------------------------------------------------
    # Unified register API
    # ------------------------------------------------------------------

    def register(
        self,
        objects: Any,
        *,
        name: str | None = None,
        runtime: str | dict[str, Any] = "inproc",
        agentcore_endpoint: str | None = None,
    ) -> RuntimeClient:
        """Register one object or a list of objects. Returns ``self`` so the
        call is chainable: ``client.register(a).register([b, mcp]).connect()``.

        Each object is dispatched by kind (swarm/graph/agent/mcp) via the
        inspector registry. Swarm/Graph instances also auto-register every
        Strands ``Agent`` they contain.

        When a list is passed, ``name`` cannot be supplied — per-object names
        are derived from ``obj.name`` or, for callers that omit ``name=``,
        from the local Python variable that holds the object.
        """
        var_names = _capture_caller_var_names(objects)

        if isinstance(objects, (list, tuple)):
            if name is not None and len(objects) > 1:
                raise ValueError(
                    "name= cannot be combined with a list of objects; "
                    "set obj.name on each instance or call register() per object"
                )
            for obj in objects:
                self._register_one(
                    obj,
                    name=name if len(objects) == 1 else None,
                    runtime=runtime,
                    agentcore_endpoint=agentcore_endpoint,
                    fallback_name=var_names.get(id(obj)),
                )
            return self
        self._register_one(
            objects,
            name=name,
            runtime=runtime,
            agentcore_endpoint=agentcore_endpoint,
            fallback_name=var_names.get(id(objects)),
        )
        return self

    def _register_one(
        self,
        obj: Any,
        *,
        name: str | None,
        runtime: str | dict[str, Any],
        agentcore_endpoint: str | None,
        fallback_name: str | None = None,
    ) -> None:
        kind = classify(obj)
        if kind is None:
            raise ValueError(
                f"register(): no inspector matched {type(obj).__name__!r}; "
                "use sideseat.runtime.adapters.register_agent_inspector(...) "
                "or call .agent(..., tools=[...]) explicitly"
            )
        derived = name or derive_default_name(obj, kind) or fallback_name
        if not derived:
            raise ValueError(
                f"register(): could not derive a name for {type(obj).__name__!r}; "
                "pass name= or set obj.name"
            )

        if kind == "agent":
            self.add_agent(
                obj,
                name=derived,
                runtime=runtime,
                agentcore_endpoint=agentcore_endpoint,
            )
            return
        if kind == "mcp":
            self.add_mcp(obj, name=derived)
            return

        # swarm / graph: register the composite, then walk inner agents.
        manifest = build_manifest_for_kind(
            kind, obj, name=derived, runtime=runtime, agentcore_endpoint=agentcore_endpoint
        )
        self._upsert_registration(_Registration(kind=kind, name=derived, manifest=manifest))
        seen: set[int] = {id(obj)}
        self._register_inner_agents(obj, runtime=runtime, seen=seen)

    def _register_inner_agents(
        self,
        container: Any,
        *,
        runtime: str | dict[str, Any],
        seen: set[int],
    ) -> None:
        nodes = getattr(container, "nodes", None)
        if not isinstance(nodes, dict):
            return
        for node_id, node in nodes.items():
            executor = getattr(node, "executor", None)
            if executor is None or id(executor) in seen:
                continue
            seen.add(id(executor))
            inner_kind = classify(executor)
            if inner_kind == "agent":
                inner_name = (
                    derive_default_name(executor, "agent")
                    or str(node_id)
                )
                self.add_agent(executor, name=inner_name, runtime=runtime)
            elif inner_kind in ("swarm", "graph"):
                inner_name = (
                    derive_default_name(executor, inner_kind)
                    or str(node_id)
                )
                manifest = build_manifest_for_kind(
                    inner_kind, executor, name=inner_name, runtime=runtime
                )
                self._upsert_registration(
                    _Registration(kind=inner_kind, name=inner_name, manifest=manifest)
                )
                self._register_inner_agents(executor, runtime=runtime, seen=seen)

    # ------------------------------------------------------------------
    # Connect / disconnect
    # ------------------------------------------------------------------

    def connect(self, *, block: bool = True, banner: bool = True) -> None:
        """Open the persistent WS and (by default) block until disconnect.

        When `block=True` (the default), this method runs until
        `disconnect()` is called or SIGINT/SIGTERM is received. The I/O
        thread is still a daemon thread; the calling thread is the one that
        blocks.

        `block=False` is intended for tests and embedding scenarios where
        the caller wants to drive its own loop and call `disconnect()`
        explicitly.

        When `block=True`, a startup banner is printed on stdout once the
        first welcome arrives. Pass `banner=False` to suppress it.
        """
        if not _module_available("websockets"):
            raise ImportError(
                "sideseat[ws] extra is not installed. "
                "Install with `pip install sideseat[ws]`."
            )
        self._banner_enabled = banner and block
        if self._banner_enabled:
            # Print "connecting" line synchronously BEFORE the I/O thread
            # starts, so the user always sees an indication of activity even
            # when the server is unreachable.
            self._print_connecting_line()
        self._ensure_thread_started()
        if not block:
            return
        self._install_signal_handlers()
        if self._banner_enabled and not self.wait_until_connected(timeout=5.0):
            self._print_unreachable_warning()
        try:
            self._stopped.wait()
        except KeyboardInterrupt:
            self.disconnect()

    def _ensure_thread_started(self) -> None:
        if self._thread is not None and self._thread.is_alive():
            return
        self._stop_event.clear()
        self._stopped.clear()
        self._thread = threading.Thread(
            target=self._run_loop,
            name="sideseat-runtime",
            daemon=True,
        )
        self._thread.start()
        if not self._atexit_registered:
            atexit.register(self.disconnect)
            self._atexit_registered = True

    def wait_until_connected(self, timeout: float = 5.0) -> bool:
        return self._connected_event.wait(timeout)

    def disconnect(self) -> None:
        if self._stopped.is_set():
            return
        # Send unregisters best-effort while still connected.
        with self._registry_lock:
            outgoing = list(self._registrations.values())
        for reg in outgoing:
            with suppress(Exception):
                self._send_envelope(make_envelope(f"{reg.kind}.unregister", {"name": reg.name}))
        self._stop_event.set()
        # Snapshot `_ws` under the send-lock so a concurrent reconnect-swap
        # cannot leave us calling `close()` on a freshly-replaced socket.
        with self._send_lock:
            ws = self._ws
        if ws is not None:
            with suppress(Exception):
                ws.close()
        # If disconnect() was called from the I/O thread itself (e.g. from a
        # `replaced` dispatch), don't join — that would deadlock for 2s.
        if (
            self._thread is not None
            and self._thread.is_alive()
            and threading.current_thread() is not self._thread
        ):
            self._thread.join(timeout=2.0)
        self._stopped.set()

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _upsert_registration(self, reg: _Registration) -> None:
        # Hold registry_lock across the wire-send so that a concurrent
        # _handle_connection re-flush sees a consistent snapshot (no
        # double-sent register, no missed entry between snapshot and append).
        with self._registry_lock:
            self._registrations[(reg.kind, reg.name)] = reg
            if self._connected_event.is_set():
                with suppress(Exception):
                    self._send_envelope(
                        make_envelope(
                            f"{reg.kind}.register",
                            reg.manifest.to_payload(),
                        )
                    )

    def _remove_registration(self, kind: str, name: str) -> None:
        with self._registry_lock:
            self._registrations.pop((kind, name), None)
            if self._connected_event.is_set():
                with suppress(Exception):
                    self._send_envelope(
                        make_envelope(f"{kind}.unregister", {"name": name})
                    )

    def _ws_url(self) -> str:
        parsed = urlparse(self._endpoint)
        scheme = "wss" if parsed.scheme == "https" else "ws"
        path = f"/api/v1/project/{self._project_id}/ws"
        return urlunparse((scheme, parsed.netloc, path, "", "", ""))

    def _run_loop(self) -> None:
        from websockets.sync.client import connect as ws_connect  # type: ignore[import-not-found]

        backoff = _RECONNECT_INITIAL
        consecutive_failures = 0

        while not self._stop_event.is_set():
            try:
                with ws_connect(
                    self._ws_url(),
                    max_size=_DEFAULT_MAX_MESSAGE_BYTES,
                    open_timeout=10,
                    close_timeout=5,
                ) as ws:
                    with self._send_lock:
                        self._ws = ws
                    consecutive_failures = 0
                    backoff = _RECONNECT_INITIAL
                    try:
                        self._handle_connection(ws)
                    finally:
                        with self._send_lock:
                            self._ws = None
                        self._connected_event.clear()
            except Exception as exc:
                consecutive_failures += 1
                if (
                    consecutive_failures >= _RECONNECT_FAILURES_LOG_THRESHOLD
                    and consecutive_failures % _RECONNECT_FAILURES_LOG_THRESHOLD == 0
                ):
                    logger.error(
                        "sideseat WS reconnect: %d consecutive failures (last: %s) endpoint=%s",
                        consecutive_failures,
                        exc,
                        self._endpoint,
                    )
                else:
                    logger.debug("WS connection failed: %s", exc)
                self._connected_event.clear()

            if self._stop_event.is_set():
                break
            sleep_for = min(_RECONNECT_MAX, backoff) * (0.7 + 0.6 * random.random())
            backoff = min(_RECONNECT_MAX, backoff * 2)
            self._stop_event.wait(sleep_for)

        self._stopped.set()

    def _handle_connection(self, ws: Any) -> None:
        # Wait for server's `welcome`, then send hello, then re-flush registry.
        welcome = self._recv_with_watchdog(ws, timeout=10.0)
        if welcome is None or welcome.type != "welcome":
            logger.debug("expected welcome, got: %r", welcome)
            return
        if isinstance(welcome.payload, dict):
            self._heartbeat_interval = int(welcome.payload.get("heartbeat_interval_secs", _DEFAULT_HEARTBEAT_INTERVAL))
        self._last_server_message_at = time.monotonic()

        self._send_envelope(
            make_envelope(
                "hello",
                {"client_id": self._client_id, "sdk_version": __version__},
            )
        )

        # Re-flush registry under the same lock that gates _upsert_registration
        # so a register racing with reconnect goes either fully before
        # `_connected_event` (caught by the snapshot) or fully after it (sent
        # live). No double-send, no missed entry.
        with self._registry_lock:
            self._connected_event.set()
            for reg in self._registrations.values():
                with suppress(Exception):
                    self._send_envelope(
                        make_envelope(
                            f"{reg.kind}.register",
                            reg.manifest.to_payload(),
                        )
                    )

        if self._banner_enabled and not self._banner_printed:
            self._banner_printed = True
            self._print_banner()

        # Drive the recv loop with a watchdog.
        deadline_extra = self._heartbeat_interval + _DEFAULT_PONG_GRACE
        while not self._stop_event.is_set():
            env = self._recv_with_watchdog(ws, timeout=1.0)
            if env is None:
                if (time.monotonic() - self._last_server_message_at) > deadline_extra:
                    logger.debug("WS watchdog: no server message in %ds, reconnecting", deadline_extra)
                    return
                continue
            self._last_server_message_at = time.monotonic()
            self._dispatch(env)

    def _recv_with_watchdog(self, ws: Any, *, timeout: float) -> Envelope | None:
        try:
            raw = ws.recv(timeout=timeout)
        except TimeoutError:
            return None
        except Exception as exc:
            logger.debug("WS recv error: %s", exc)
            raise
        if raw is None:
            return None
        if isinstance(raw, (bytes, bytearray)):
            raw = raw.decode("utf-8", errors="replace")
        try:
            return parse_envelope(raw)
        except Exception as exc:
            logger.debug("envelope parse error: %s (raw=%r)", exc, raw)
            return None

    def _dispatch(self, env: Envelope) -> None:
        if env.type == "ping":
            with suppress(Exception):
                self._send_envelope(
                    make_envelope("pong", {"id": env.id})
                )
        elif env.type == "ack":
            logger.debug("ack ref_id=%s", _payload_field(env, "ref_id"))
        elif env.type == "error":
            code = _payload_field(env, "code")
            msg = _payload_field(env, "message")
            if code in (ErrorCode.HELLO_REQUIRED.value, ErrorCode.REPLACED.value):
                logger.warning("server error %s: %s", code, msg)
            else:
                logger.debug("server error %s: %s", code, msg)
        elif env.type == "replaced":
            logger.info(
                "registration replaced for %s/%s; disconnecting",
                _payload_field(env, "kind"),
                _payload_field(env, "name"),
            )
            self.disconnect()
        else:
            logger.debug("unknown frame: %s", env.type)

    def _send_envelope(self, env: Envelope) -> None:
        # `_send_lock` serialises writes AND protects the `self._ws` read so a
        # concurrent reconnect/disconnect cannot swap the socket out from under
        # an in-flight `send()`. The lock is uncontended on the steady-state
        # send path (recv-loop pongs + occasional register frames).
        with self._send_lock:
            ws = self._ws
            if ws is None:
                return
            ws.send(env.to_json())

    def _print_connecting_line(self) -> None:
        """One-line note printed BEFORE the I/O thread starts so the user
        always sees something on stdout — even if the server is unreachable
        and we never make it past reconnect-loop."""
        with self._registry_lock:
            registrations = list(self._registrations.values())
        kinds_summary = ", ".join(
            f"{kind} ({len([r for r in registrations if r.kind == kind])})"
            for kind in ("agent", "swarm", "graph", "mcp")
            if any(r.kind == kind for r in registrations)
        ) or "no registrations"
        print(
            f"SideSeat: connecting to {self._endpoint} "
            f"(project={self._project_id}, {kinds_summary}) ...",
            flush=True,
        )

    def _print_unreachable_warning(self) -> None:
        print(
            f"SideSeat: still not connected after 5s -- is the server running at "
            f"{self._endpoint}? Retrying in the background. "
            "Press Ctrl-C to abort.",
            flush=True,
        )

    def _print_banner(self) -> None:
        """Print a friendly box on stdout listing what's been registered.

        Stays ASCII-only and never colorises — the host terminal may not
        support either, and CLAUDE.md forbids emojis in console output.
        """
        with self._registry_lock:
            registrations = list(self._registrations.values())
        by_kind: dict[str, list[_Registration]] = {}
        for reg in registrations:
            by_kind.setdefault(reg.kind, []).append(reg)

        lines: list[str] = []
        lines.append("SideSeat presence connected")
        lines.append(f"  endpoint   : {self._endpoint}")
        lines.append(f"  project    : {self._project_id}")
        lines.append(f"  client_id  : {self._client_id}")

        listing_base = self._endpoint.rstrip("/")
        lines.append(
            f"  listing    : {listing_base}/api/v1/project/{self._project_id}/registrations"
        )

        if registrations:
            lines.append("")
            lines.append("Registered:")
            for kind in ("agent", "swarm", "graph", "mcp"):
                items = by_kind.get(kind)
                if not items:
                    continue
                names = ", ".join(sorted(r.name for r in items))
                lines.append(f"  {kind:<10s} ({len(items)}): {names}")
        else:
            lines.append("")
            lines.append(
                "No registrations yet. Call client.register(...) before connect()."
            )
        lines.append("")
        lines.append("Press Ctrl-C to disconnect.")

        width = max(len(line) for line in lines)
        bar = "+" + "-" * (width + 2) + "+"
        framed = [bar]
        for line in lines:
            framed.append(f"| {line.ljust(width)} |")
        framed.append(bar)
        print("\n".join(framed), flush=True)

    def _install_signal_handlers(self) -> None:
        # `signal.signal()` only works on the main thread of the main
        # interpreter; bail out silently otherwise.
        if (
            self._signal_handlers_installed
            or threading.current_thread() is not threading.main_thread()
        ):
            return
        self._signal_handlers_installed = True
        for sig in (signal.SIGINT, signal.SIGTERM):
            try:
                current = signal.getsignal(sig)
            except (ValueError, OSError):
                continue
            # Preserve user-installed handlers; only install if at default.
            if current in (signal.SIG_DFL, None):
                with suppress(ValueError, OSError):
                    signal.signal(sig, lambda *_a: self.disconnect())


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
        outer = sys._getframe(2)  # type: ignore[attr-defined]
    except (AttributeError, ValueError):
        return {}

    targets: list[Any] = list(arg) if isinstance(arg, (list, tuple)) else [arg]
    target_ids = {id(t) for t in targets}
    found: dict[int, str] = {}

    frame = outer
    seen_frames = 0
    while frame is not None and seen_frames < 20 and target_ids:
        for var_name, var_val in frame.f_locals.items():
            vid = id(var_val)
            if vid in target_ids and vid not in found:
                found[vid] = var_name
        target_ids -= set(found.keys())
        frame = frame.f_back
        seen_frames += 1
    return found


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


__all__ = ["RuntimeClient", "PROTOCOL_VERSION"]
