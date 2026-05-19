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
from collections.abc import Iterator
from contextlib import contextmanager
from typing import Any

import pytest

websockets = pytest.importorskip("websockets")
from websockets.sync.server import (  # type: ignore[import-not-found]  # noqa: E402
    ServerConnection,
    serve,
)

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
            assert err["payload"]["code"] == "registration_not_found"
        finally:
            client.disconnect()


def test_invoke_graph_routes_to_multiagent_converter() -> None:
    """A registered Strands Graph is invoked by name; the multiagent
    converter drives `stream_async` and emits per-node STEP_STARTED /
    STEP_FINISHED brackets plus content."""

    class _Node:
        def __init__(self, executor: Any) -> None:
            self.executor = executor

    class _Graph:
        __module__ = "strands.multiagent.graph"

        def __init__(self) -> None:
            inner = _StubAgent(name="inner")
            self.name = "pipeline"
            self.nodes = {"a": _Node(inner), "b": _Node(inner)}

        async def stream_async(self, prompt: str):  # noqa: ARG002
            yield {"type": "multiagent_node_start", "node_id": "a"}
            yield {
                "type": "multiagent_node_stream",
                "node_id": "a",
                "event": {"data": "hello-from-a"},
            }
            yield {"type": "multiagent_node_stop", "node_id": "a"}
            yield {"type": "multiagent_node_start", "node_id": "b"}
            yield {
                "type": "multiagent_node_stream",
                "node_id": "b",
                "event": {"data": "hello-from-b"},
            }
            yield {"type": "multiagent_node_stop", "node_id": "b"}

    _Graph.__name__ = "Graph"

    with stub_server() as srv:
        srv.script([_invoke("req-graph", agent_name="pipeline")])
        client = RuntimeClient(endpoint=f"http://127.0.0.1:{srv.port}", project_id="default")
        try:
            graph = _Graph()
            client.register(graph, name="pipeline")
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)

            assert srv.first_of_type("agent.complete", timeout=10.0) is not None
            event_payloads = [
                f["payload"]["event"]
                for f in srv.all_of_type("agent.event")
                if "event" in f.get("payload", {})
            ]
            types = [p.get("type") for p in event_payloads]
            assert types.count("RUN_STARTED") == 1
            assert types.count("RUN_FINISHED") == 1
            # One STEP_STARTED + one STEP_FINISHED per node, with the node_id
            # surfacing as step_name.
            step_starts = [p for p in event_payloads if p.get("type") == "STEP_STARTED"]
            step_names = [p.get("stepName") or p.get("step_name") for p in step_starts]
            assert step_names == ["a", "b"]
            # Content from each node arrives between its STEP brackets.
            text_deltas = [
                p.get("delta") for p in event_payloads if p.get("type") == "TEXT_MESSAGE_CONTENT"
            ]
            assert "hello-from-a" in text_deltas
            assert "hello-from-b" in text_deltas
        finally:
            client.disconnect()


def test_invoke_swarm_routes_to_multiagent_converter() -> None:
    """A registered Strands Swarm goes through the same multiagent
    converter as a Graph — both kinds use `stream_async` and emit
    `multiagent_node_*` events."""

    class _Node:
        def __init__(self, executor: Any) -> None:
            self.executor = executor

    class _Swarm:
        __module__ = "strands.multiagent.swarm"

        def __init__(self) -> None:
            self.name = "team"
            self.nodes = {"alice": _Node(_StubAgent(name="alice"))}

        async def stream_async(self, prompt: str):  # noqa: ARG002
            yield {"type": "multiagent_node_start", "node_id": "alice"}
            yield {
                "type": "multiagent_node_stream",
                "node_id": "alice",
                "event": {"data": "thinking"},
            }
            yield {"type": "multiagent_node_stop", "node_id": "alice"}

    _Swarm.__name__ = "Swarm"

    with stub_server() as srv:
        srv.script([_invoke("req-swarm", agent_name="team")])
        client = RuntimeClient(endpoint=f"http://127.0.0.1:{srv.port}", project_id="default")
        try:
            client.register(_Swarm(), name="team")
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)

            assert srv.first_of_type("agent.complete", timeout=10.0) is not None
            payloads = [
                f["payload"]["event"]
                for f in srv.all_of_type("agent.event")
                if "event" in f.get("payload", {})
            ]
            types = [p.get("type") for p in payloads]
            assert types.count("RUN_STARTED") == 1
            assert types.count("RUN_FINISHED") == 1
            assert "STEP_STARTED" in types
            assert "STEP_FINISHED" in types
        finally:
            client.disconnect()


def test_invoke_busy_check_scoped_by_kind_name() -> None:
    """The busy-slot must be keyed by `(kind, name)` — a graph invoke must
    NOT block while an unrelated agent invoke is mid-stream. Verifies
    `_busy_agents` is `set[(kind, name)]`, not `set[str]`."""
    import asyncio

    # Slow inner agent: parks on `asyncio.sleep` so the second (graph)
    # invoke arrives while this one still owns the agent slot.
    from ag_ui.core import (
        RunFinishedEvent,
        RunStartedEvent,
        TextMessageContentEvent,
        TextMessageEndEvent,
        TextMessageStartEvent,
    )

    class _SlowAgentRun:
        def __init__(self, *, agent: Any, name: str, description: str = "") -> None:
            self._a = agent
            self._n = name

        async def run(self, ri: Any):  # type: ignore[no-untyped-def]
            yield RunStartedEvent(thread_id=ri.thread_id, run_id=ri.run_id)
            yield TextMessageStartEvent(message_id="m", role="assistant")
            await asyncio.sleep(2.0)  # park; the graph runs concurrently
            yield TextMessageContentEvent(message_id="m", delta="ok")
            yield TextMessageEndEvent(message_id="m")
            yield RunFinishedEvent(thread_id=ri.thread_id, run_id=ri.run_id)

    sys.modules["ag_ui_strands"].StrandsAgent = _SlowAgentRun  # type: ignore[attr-defined]

    class _Node:
        def __init__(self, executor: Any) -> None:
            self.executor = executor

    class _Graph:
        __module__ = "strands.multiagent.graph"

        def __init__(self, inner: Any) -> None:
            self.name = "pipeline"
            self.nodes = {"a": _Node(inner)}

        async def stream_async(self, prompt: str):  # noqa: ARG002
            yield {"type": "multiagent_node_start", "node_id": "a"}
            yield {"type": "multiagent_node_stop", "node_id": "a"}

    _Graph.__name__ = "Graph"

    with stub_server() as srv:
        srv.script(
            [
                _invoke("req-agent", agent_name="weather"),
                _invoke("req-graph", agent_name="pipeline"),
            ]
        )
        client = RuntimeClient(endpoint=f"http://127.0.0.1:{srv.port}", project_id="default")
        try:
            inner = _StubAgent(name="weather")
            client.add_agent(inner, name="weather")
            graph = _Graph(inner)
            client.register(graph, name="pipeline")
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)

            # Graph completes on its own slot while the agent invoke is
            # still parked on asyncio.sleep — proves slots are independent.
            graph_done = srv.first_of_type("agent.complete", timeout=5.0)
            assert graph_done is not None
            assert graph_done["payload"]["request_id"] == "req-graph"

            # No spurious agent_busy/registration_not_found error frames.
            errors = srv.all_of_type("agent.error")
            assert not errors, f"unexpected errors: {errors}"
        finally:
            client.disconnect()


def test_invoke_mcp_returns_unsupported_backend() -> None:
    """An mcp registration is reachable by name but cannot be invoked
    via the AG-UI run-agent path."""

    class _MCP:
        def __init__(self) -> None:
            self.name = "tools_server"

    with stub_server() as srv:
        srv.script([_invoke("req-mcp", agent_name="tools_server")])
        client = RuntimeClient(endpoint=f"http://127.0.0.1:{srv.port}", project_id="default")
        try:
            # Pass an explicit tools list so add_mcp can build a
            # manifest without an inspector match.
            client.add_mcp(_MCP(), name="tools_server", tools=[{"name": "noop"}])
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)
            err = srv.first_of_type("agent.error", timeout=5.0)
            assert err is not None
            assert err["payload"]["code"] == "unsupported_backend"
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
