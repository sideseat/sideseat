"""End-to-end tests for the sync RuntimeClient.

Uses `websockets.sync.server` to run a stub server in a daemon thread.
"""

from __future__ import annotations

import json
import socket
import threading
import time
from collections.abc import Iterator
from contextlib import contextmanager
from typing import Any

import pytest

websockets = pytest.importorskip("websockets")
from websockets.sync.server import (  # type: ignore[import-not-found]  # noqa: E402
    ServerConnection,
    serve,
)

from sideseat.runtime.client import RuntimeClient  # noqa: E402
from sideseat.runtime.protocol import make_envelope  # noqa: E402


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class _StubServer:
    def __init__(self, port: int | None = None) -> None:
        self.port = port if port is not None else _free_port()
        self.frames: list[dict[str, Any]] = []
        self.lock = threading.Lock()
        self.connect_count = 0
        self._server = None
        self._thread: threading.Thread | None = None
        self._handler_started = threading.Event()

    def _handle(self, conn: ServerConnection) -> None:
        with self.lock:
            self.connect_count += 1
        self._handler_started.set()
        try:
            conn.send(
                make_envelope(
                    "welcome",
                    {
                        "connection_id": "stub-conn",
                        "server_version": "test",
                        "max_message_bytes": 4 * 1024 * 1024,
                        "heartbeat_interval_secs": 20,
                    },
                ).to_json()
            )
            for raw in conn:
                if isinstance(raw, (bytes, bytearray)):
                    raw = raw.decode("utf-8")
                data = json.loads(raw)
                with self.lock:
                    self.frames.append(data)
                if data["type"] == "hello":
                    conn.send(make_envelope("ack", {"ref_id": data["id"]}).to_json())
                elif data["type"] in (
                    "agent.register",
                    "mcp.register",
                    "agent.unregister",
                    "mcp.unregister",
                ):
                    conn.send(make_envelope("ack", {"ref_id": data["id"]}).to_json())
        except Exception:
            pass

    def start(self) -> None:
        self._server = serve(self._handle, "127.0.0.1", self.port)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()

    def stop(self) -> None:
        if self._server is not None:
            self._server.shutdown()
            self._server = None

    def first_frame_of_type(self, t: str, timeout: float = 5.0) -> dict[str, Any] | None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.lock:
                for f in self.frames:
                    if f.get("type") == t:
                        return f
            time.sleep(0.05)
        return None


@contextmanager
def stub_server() -> Iterator[_StubServer]:
    s = _StubServer()
    s.start()
    try:
        yield s
    finally:
        s.stop()


def test_connect_sends_hello_then_register() -> None:
    with stub_server() as srv:
        client = RuntimeClient(
            endpoint=f"http://127.0.0.1:{srv.port}",
            project_id="default",
        )
        try:
            client.add_agent(
                _DummyStrandsAgent(),
                name="weather",
                runtime="inproc",
            )
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)

            hello = srv.first_frame_of_type("hello")
            assert hello is not None
            assert hello["payload"]["client_id"]
            assert hello["payload"]["sdk_version"]

            reg = srv.first_frame_of_type("agent.register")
            assert reg is not None
            assert reg["payload"]["name"] == "weather"
        finally:
            client.disconnect()


def test_disconnect_sends_unregisters() -> None:
    with stub_server() as srv:
        client = RuntimeClient(
            endpoint=f"http://127.0.0.1:{srv.port}",
            project_id="default",
        )
        client.add_agent(_DummyStrandsAgent(), name="weather", runtime="inproc")
        client.connect(block=False)
        assert client.wait_until_connected(timeout=5)
        # Wait for register frame before disconnecting so we don't race the
        # I/O thread on send-flush.
        assert srv.first_frame_of_type("agent.register", timeout=5.0) is not None
        client.disconnect()

        unreg = srv.first_frame_of_type("agent.unregister", timeout=5.0)
        assert unreg is not None
        assert unreg["payload"]["name"] == "weather"


def test_reconnect_re_sends_same_client_id() -> None:
    """Closing the server-side socket triggers a reconnect that re-uses the
    same client_id, so the server can detect 'same owner' on upsert."""
    close_after_first = threading.Event()
    second_handle = threading.Event()

    def handler(conn: ServerConnection) -> None:
        # Welcome.
        conn.send(
            make_envelope(
                "welcome",
                {
                    "connection_id": "stub-conn",
                    "server_version": "test",
                    "max_message_bytes": 4 * 1024 * 1024,
                    "heartbeat_interval_secs": 20,
                },
            ).to_json()
        )
        captured: list[dict[str, Any]] = []
        for raw in conn:
            data = json.loads(raw if isinstance(raw, str) else raw.decode("utf-8"))
            captured.append(data)
            if data["type"] == "hello":
                conn.send(make_envelope("ack", {"ref_id": data["id"]}).to_json())
                if not close_after_first.is_set():
                    close_after_first.set()
                    captures_box["first"] = data["payload"]["client_id"]
                    return  # close socket; client should reconnect
                else:
                    captures_box["second"] = data["payload"]["client_id"]
                    second_handle.set()
                    return

    captures_box: dict[str, str] = {}
    port = _free_port()
    server = serve(handler, "127.0.0.1", port)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()

    client = RuntimeClient(endpoint=f"http://127.0.0.1:{port}", project_id="default")
    try:
        client.add_agent(_DummyStrandsAgent(), name="weather", runtime="inproc")
        client.connect(block=False)
        assert second_handle.wait(timeout=10.0), "second hello never arrived"
        assert captures_box["first"] == captures_box["second"]
    finally:
        client.disconnect()
        server.shutdown()


def test_register_dispatches_by_kind_with_chaining() -> None:
    """`register()` accepts a single object or a list, classifies each one,
    and is chainable."""
    with stub_server() as srv:
        client = RuntimeClient(
            endpoint=f"http://127.0.0.1:{srv.port}",
            project_id="default",
        )
        try:
            agent = _DummyStrandsAgent()
            agent.name = "weather"  # used as the registration identity
            mcp = _DummyMcpClient()
            mcp.name = "search-mcp"

            ret_a = client.register(agent)
            ret_b = client.register([mcp])
            assert ret_a is client and ret_b is client  # chainable
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)

            reg_agent = srv.first_frame_of_type("agent.register", timeout=5.0)
            reg_mcp = srv.first_frame_of_type("mcp.register", timeout=5.0)
            assert reg_agent is not None and reg_agent["payload"]["name"] == "weather"
            assert reg_mcp is not None and reg_mcp["payload"]["name"] == "search-mcp"
        finally:
            client.disconnect()


def test_register_accepts_single_object_and_list_equivalently() -> None:
    """`register(swarm)` and `register([swarm])` produce the same wire frames."""

    class _Node:
        def __init__(self, node_id: str, executor: Any) -> None:
            self.node_id = node_id
            self.executor = executor

    class _Swarm:
        __module__ = "strands.multiagent.swarm"

        def __init__(self, name: str) -> None:
            self.name = name
            inner = _DummyStrandsAgent()
            inner.name = "inner"
            self.nodes = {"inner": _Node("inner", inner)}

    _Swarm.__name__ = "Swarm"

    # Variant A: single object.
    with stub_server() as srv:
        client = RuntimeClient(endpoint=f"http://127.0.0.1:{srv.port}", project_id="default")
        try:
            client.register(_Swarm("single"))
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)
            assert srv.first_frame_of_type("swarm.register", timeout=5.0) is not None
            assert srv.first_frame_of_type("agent.register", timeout=5.0) is not None
        finally:
            client.disconnect()

    # Variant B: same call shape via a list.
    with stub_server() as srv:
        client = RuntimeClient(endpoint=f"http://127.0.0.1:{srv.port}", project_id="default")
        try:
            client.register([_Swarm("listed")])
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)
            assert srv.first_frame_of_type("swarm.register", timeout=5.0) is not None
            assert srv.first_frame_of_type("agent.register", timeout=5.0) is not None
        finally:
            client.disconnect()


def test_register_swarm_also_registers_inner_agents() -> None:
    with stub_server() as srv:
        client = RuntimeClient(
            endpoint=f"http://127.0.0.1:{srv.port}",
            project_id="default",
        )
        try:

            class _Node:
                def __init__(self, node_id: str, executor: Any) -> None:
                    self.node_id = node_id
                    self.executor = executor

            class _Swarm:
                __module__ = "strands.multiagent.swarm"
                name = "research-swarm"

                def __init__(self, agents: dict[str, Any]) -> None:
                    self.nodes = {nid: _Node(nid, a) for nid, a in agents.items()}

            _Swarm.__name__ = "Swarm"

            alice = _DummyStrandsAgent()
            alice.name = "alice"
            bob = _DummyStrandsAgent()
            bob.name = "bob"
            swarm = _Swarm({"alice": alice, "bob": bob})

            client.register([swarm])
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)

            swarm_reg = srv.first_frame_of_type("swarm.register", timeout=5.0)
            assert swarm_reg is not None
            assert swarm_reg["payload"]["name"] == "research-swarm"
            assert swarm_reg["payload"]["framework"] == "strands-python"

            # Both inner agents should be registered separately.
            time.sleep(0.4)
            agent_frames = [f for f in srv.frames if f.get("type") == "agent.register"]
            agent_names = {f["payload"]["name"] for f in agent_frames}
            assert {"alice", "bob"}.issubset(agent_names)
        finally:
            client.disconnect()


def test_register_falls_back_to_local_variable_name() -> None:
    """When obj.name is missing, register() picks the local variable name."""
    with stub_server() as srv:
        client = RuntimeClient(
            endpoint=f"http://127.0.0.1:{srv.port}",
            project_id="default",
        )
        try:
            my_helper = _DummyStrandsAgent()
            # Strip any inherited name attribute.
            if hasattr(my_helper, "name"):
                del my_helper.name  # type: ignore[attr-defined]
            client.register(my_helper)
            client.connect(block=False)
            assert client.wait_until_connected(timeout=5)

            reg = srv.first_frame_of_type("agent.register", timeout=5.0)
            assert reg is not None
            assert reg["payload"]["name"] == "my_helper"
        finally:
            client.disconnect()


def test_replaced_frame_does_not_deadlock() -> None:
    """A `replaced` notice arriving on the I/O thread triggers
    self.disconnect(), which re-enters the registry/send locks. RLock makes
    that safe."""
    replaced_sent = threading.Event()

    def handler(conn: ServerConnection) -> None:
        conn.send(
            make_envelope(
                "welcome",
                {
                    "connection_id": "stub",
                    "server_version": "test",
                    "max_message_bytes": 4 * 1024 * 1024,
                    "heartbeat_interval_secs": 20,
                },
            ).to_json()
        )
        for raw in conn:
            data = json.loads(raw if isinstance(raw, str) else raw.decode("utf-8"))
            if data["type"] == "hello":
                conn.send(make_envelope("ack", {"ref_id": data["id"]}).to_json())
            elif data["type"].endswith(".register"):
                conn.send(make_envelope("ack", {"ref_id": data["id"]}).to_json())
                conn.send(
                    make_envelope(
                        "replaced", {"kind": "agent", "name": data["payload"]["name"]}
                    ).to_json()
                )
                replaced_sent.set()
            elif data["type"].endswith(".unregister"):
                conn.send(make_envelope("ack", {"ref_id": data["id"]}).to_json())
                return

    port = _free_port()
    server = serve(handler, "127.0.0.1", port)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()

    client = RuntimeClient(endpoint=f"http://127.0.0.1:{port}", project_id="default")
    try:
        agent = _DummyStrandsAgent()
        agent.name = "x"
        client.register([agent])
        client.connect(block=False)
        # Within 5s the client must have processed `replaced` and self-stopped.
        for _ in range(50):
            if client._stopped.is_set():
                break
            time.sleep(0.1)
        assert replaced_sent.is_set()
        assert client._stopped.is_set(), "client must self-disconnect on replaced"
    finally:
        client.disconnect()
        server.shutdown()


def test_register_unknown_object_raises() -> None:
    rc = RuntimeClient(endpoint="http://127.0.0.1:1", project_id="default")
    with pytest.raises(ValueError, match="no inspector matched"):
        rc.register(object())


def test_register_names_overrides_per_object() -> None:
    """`names=[a, b]` assigns per-object names when registering a list;
    `name=` is rejected for a list and `names=` for a single object."""
    rc = RuntimeClient(endpoint="http://127.0.0.1:1", project_id="default")
    a = _DummyStrandsAgent()
    b = _DummyStrandsAgent()

    rc.register([a, b], names=["alpha", "beta"])
    keys = set(rc._registrations.keys())
    assert ("agent", "alpha") in keys
    assert ("agent", "beta") in keys

    with pytest.raises(ValueError, match="name= is only valid for a single object"):
        rc.register([_DummyStrandsAgent()], name="x")

    with pytest.raises(ValueError, match="names= length"):
        rc.register([_DummyStrandsAgent(), _DummyStrandsAgent()], names=["only-one"])

    with pytest.raises(ValueError, match="names= is only valid for a list"):
        rc.register(_DummyStrandsAgent(), names=["x"])


def test_register_name_collision_across_kinds_raises() -> None:
    """Names must be globally unique across kinds — the server resolves
    by name only, so an agent and graph sharing a name would silently
    shadow each other otherwise."""
    rc = RuntimeClient(endpoint="http://127.0.0.1:1", project_id="default")
    agent = _DummyStrandsAgent()
    agent.name = "shared"
    rc.register(agent)

    class _Node:
        def __init__(self, executor: Any) -> None:
            self.executor = executor

    class _Graph:
        __module__ = "strands.multiagent.graph"
        name = "shared"

        def __init__(self) -> None:
            self.nodes = {"a": _Node(_DummyStrandsAgent())}

    _Graph.__name__ = "Graph"

    with pytest.raises(ValueError, match="already registered as 'agent'"):
        rc.register(_Graph())


def test_connect_without_ws_extra_raises_clear_error(monkeypatch: pytest.MonkeyPatch) -> None:
    from sideseat.runtime import client as client_module

    monkeypatch.setattr(client_module, "_module_available", lambda _name: False)
    rc = RuntimeClient(endpoint="http://127.0.0.1:1", project_id="default")
    with pytest.raises(ImportError, match="sideseat\\[ws\\]"):
        rc.connect(block=False)


# ---------------------------------------------------------------------------
# Stub Strands agent
# ---------------------------------------------------------------------------


class _ToolRegistry:
    def get_all_tools_config(self) -> list[dict[str, Any]]:
        return [{"name": "noop"}]


class _DummyStrandsAgent:
    __module__ = "strands.agent.agent"

    def __init__(self) -> None:
        self.tool_registry = _ToolRegistry()
        self.system_prompt = None
        self.model = "stub"


_DummyStrandsAgent.__name__ = "Agent"


class _DummyMcpClient:
    def list_tools_sync(self) -> list[dict[str, Any]]:
        return [{"name": "search"}]
