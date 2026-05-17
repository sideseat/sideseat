"""Tests for the v2 invoke flow (server→SDK agent.invoke handling).

These tests stub `ag_ui_strands.StrandsAgent` (which lives only in the
upstream monorepo subdirectory and isn't on PyPI) so they can run in CI
without the optional dependency. End-to-end verification is in the live
sample, not here.
"""

from __future__ import annotations

import json
import socket
import sys
import threading
import time
import types
from contextlib import contextmanager
from typing import Any, Iterator

import pytest

websockets = pytest.importorskip("websockets")
from websockets.sync.server import ServerConnection, serve  # type: ignore[import-not-found]


# ---------------------------------------------------------------------------
# Stub ag_ui_strands so the tests don't need the upstream package.
# Done at import time, BEFORE we import from sideseat.
# ---------------------------------------------------------------------------


def _ensure_ag_ui_strands_stub() -> None:
    if "ag_ui_strands" in sys.modules:
        return
    pytest.importorskip("ag_ui.core")  # ag_ui_protocol must be present
    from ag_ui.core import (
        RunFinishedEvent,
        RunStartedEvent,
        TextMessageContentEvent,
        TextMessageEndEvent,
        TextMessageStartEvent,
    )

    class _StubStrandsAgent:
        def __init__(self, *, agent: Any, name: str, description: str = "") -> None:
            self._agent = agent
            self._name = name

        async def run(self, run_input: Any):  # type: ignore[no-untyped-def]
            yield RunStartedEvent(thread_id=run_input.thread_id, run_id=run_input.run_id)
            yield TextMessageStartEvent(message_id="m1", role="assistant")
            yield TextMessageContentEvent(message_id="m1", delta="Hello")
            yield TextMessageEndEvent(message_id="m1")
            yield RunFinishedEvent(thread_id=run_input.thread_id, run_id=run_input.run_id)

    mod = types.ModuleType("ag_ui_strands")
    mod.StrandsAgent = _StubStrandsAgent  # type: ignore[attr-defined]
    sys.modules["ag_ui_strands"] = mod


_ensure_ag_ui_strands_stub()

from sideseat.runtime.client import RuntimeClient  # noqa: E402


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


# ---------------------------------------------------------------------------
# Stub Strands Agent. Has to look like strands.Agent enough that the
# SideSeat inspector recognises it AND _run_invoke_async can swap its
# callback_handler.
# ---------------------------------------------------------------------------


class _ToolRegistry:
    def get_all_tools_config(self) -> dict[str, Any]:
        return {}


class _StubAgent:
    __module__ = "strands.agent.agent"

    def __init__(self, *, name: str = "weather") -> None:
        self.name = name
        self.tool_registry = _ToolRegistry()
        self.system_prompt = "stub"
        self.model = "stub"
        self.callback_handler = None
        self._cancel_called = False

    def cancel(self) -> None:
        self._cancel_called = True


_StubAgent.__name__ = "Agent"


# ---------------------------------------------------------------------------
# Stub WS server.
# ---------------------------------------------------------------------------


class _StubServer:
    def __init__(self) -> None:
        self.port = _free_port()
        self.frames: list[dict] = []
        self.lock = threading.Lock()
        self._server = None
        self._scripted: list[str] = []

    def script(self, frames: list[dict]) -> None:
        self._scripted = [json.dumps(f) for f in frames]

    def _handle(self, conn: ServerConnection) -> None:
        try:
            conn.send(
                json.dumps(
                    {
                        "v": 1,
                        "type": "welcome",
                        "id": "w1",
                        "payload": {
                            "connection_id": "stub",
                            "server_version": "test",
                            "max_message_bytes": 4 * 1024 * 1024,
                            "heartbeat_interval_secs": 20,
                        },
                    }
                )
            )
            for raw in conn:
                data = json.loads(raw if isinstance(raw, str) else raw.decode("utf-8"))
                with self.lock:
                    self.frames.append(data)
                if data.get("type") == "hello":
                    conn.send(
                        json.dumps(
                            {"v": 1, "type": "ack", "id": "a", "payload": {"ref_id": data["id"]}}
                        )
                    )
                    for s in self._scripted:
                        conn.send(s)
                elif data.get("type", "").endswith(".register") or data.get("type", "").endswith(
                    ".unregister"
                ):
                    conn.send(
                        json.dumps(
                            {"v": 1, "type": "ack", "id": "a", "payload": {"ref_id": data["id"]}}
                        )
                    )
        except Exception:
            pass

    def start(self) -> None:
        # `max_size` mirrors the production server's WS frame cap so the
        # chunking test doesn't trip the websockets default 1 MiB cap.
        # We use a slightly oversized cap to leave headroom for envelope
        # framing when chunked payloads sit close to the threshold.
        self._server = serve(self._handle, "127.0.0.1", self.port, max_size=8 * 1024 * 1024)
        threading.Thread(target=self._server.serve_forever, daemon=True).start()

    def stop(self) -> None:
        if self._server is not None:
            self._server.shutdown()
            self._server = None

    def first_of_type(self, t: str, timeout: float = 5.0) -> dict | None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.lock:
                for f in self.frames:
                    if f.get("type") == t:
                        return f
            time.sleep(0.05)
        return None

    def all_of_type(self, t: str) -> list[dict]:
        with self.lock:
            return [f for f in self.frames if f.get("type") == t]


@contextmanager
def stub_server() -> Iterator[_StubServer]:
    s = _StubServer()
    s.start()
    try:
        yield s
    finally:
        s.stop()


def _invoke(request_id: str, agent_name: str = "weather") -> dict:
    return {
        "v": 1,
        "type": "agent.invoke",
        "id": "inv",
        "payload": {
            "request_id": request_id,
            "agent_name": agent_name,
            "run_input": {
                "thread_id": "t1",
                "run_id": "r1",
                "state": {},
                "messages": [{"id": "m1", "role": "user", "content": "Hi"}],
                "tools": [],
                "context": [],
                "forwarded_props": {},
            },
        },
    }


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_invoke_streams_agui_events_and_completes() -> None:
    with stub_server() as srv:
        srv.script([_invoke("req-1")])
        client = RuntimeClient(endpoint=f"http://127.0.0.1:{srv.port}", project_id="default")
        try:
            client.add_agent(_StubAgent(), name="weather")
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)

            assert srv.first_of_type("agent.complete", timeout=5.0) is not None
            events = [
                f["payload"]["event"].get("type")
                for f in srv.all_of_type("agent.event")
                if "event" in f.get("payload", {})
            ]
            assert "RUN_STARTED" in events
            assert "TEXT_MESSAGE_CONTENT" in events
            assert "RUN_FINISHED" in events
        finally:
            client.disconnect()


def test_invoke_for_unregistered_agent_returns_error() -> None:
    with stub_server() as srv:
        srv.script([_invoke("req-x", agent_name="ghost")])
        client = RuntimeClient(endpoint=f"http://127.0.0.1:{srv.port}", project_id="default")
        try:
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)
            err = srv.first_of_type("agent.error", timeout=5.0)
            assert err is not None
            assert err["payload"]["code"] == "agent_not_registered"
        finally:
            client.disconnect()


def test_large_agui_event_splits_into_chunks() -> None:
    """An AG-UI event whose serialised JSON is bigger than the chunk
    threshold goes out as `agent.event.chunk` frames in order, under
    one send-lock acquisition."""
    big_delta = "X" * (4 * 1024 * 1024)  # 4 MiB string → forces chunking

    # Stub yields one giant TEXT_MESSAGE_CONTENT.
    from ag_ui.core import (
        RunFinishedEvent,
        RunStartedEvent,
        TextMessageContentEvent,
        TextMessageEndEvent,
        TextMessageStartEvent,
    )

    class _BigSA:
        def __init__(self, *, agent: Any, name: str, description: str = "") -> None:
            self._a = agent

        async def run(self, ri: Any):  # type: ignore[no-untyped-def]
            yield RunStartedEvent(thread_id=ri.thread_id, run_id=ri.run_id)
            yield TextMessageStartEvent(message_id="m1", role="assistant")
            yield TextMessageContentEvent(message_id="m1", delta=big_delta)
            yield TextMessageEndEvent(message_id="m1")
            yield RunFinishedEvent(thread_id=ri.thread_id, run_id=ri.run_id)

    sys.modules["ag_ui_strands"].StrandsAgent = _BigSA  # type: ignore[attr-defined]

    with stub_server() as srv:
        srv.script([_invoke("req-big")])
        client = RuntimeClient(endpoint=f"http://127.0.0.1:{srv.port}", project_id="default")
        try:
            client.add_agent(_StubAgent(), name="weather")
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)

            # Wait for the whole exchange to land. 4 MiB chunked over WS
            # is slow; give the loopback socket some real time.
            complete = srv.first_of_type("agent.complete", timeout=30.0)
            if complete is None:
                # Diagnostic dump.
                with srv.lock:
                    types_seen = [f.get("type") for f in srv.frames]
                raise AssertionError(
                    f"agent.complete not seen in 30s; frames seen: {types_seen[:30]}..."
                    f" total={len(types_seen)}"
                )

            chunks = srv.all_of_type("agent.event.chunk")
            assert len(chunks) >= 2, "big event must produce ≥ 2 chunks"

            # Same group_id, monotonic idx, all carry the same total.
            payloads = [c["payload"] for c in chunks]
            group_ids = {p["group_id"] for p in payloads}
            assert len(group_ids) == 1
            totals = {p["total"] for p in payloads}
            assert totals == {len(chunks)}
            assert [p["idx"] for p in payloads] == list(range(len(chunks)))
        finally:
            client.disconnect()


def test_invoke_bad_run_input_returns_bad_run_input() -> None:
    bad = {
        "v": 1,
        "type": "agent.invoke",
        "id": "inv",
        "payload": {
            "request_id": "req-bad",
            "agent_name": "weather",
            "run_input": {},  # missing required fields
        },
    }
    with stub_server() as srv:
        srv.script([bad])
        client = RuntimeClient(endpoint=f"http://127.0.0.1:{srv.port}", project_id="default")
        try:
            client.add_agent(_StubAgent(), name="weather")
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)
            err = srv.first_of_type("agent.error", timeout=5.0)
            assert err is not None
            assert err["payload"]["code"] == "bad_run_input"
        finally:
            client.disconnect()
