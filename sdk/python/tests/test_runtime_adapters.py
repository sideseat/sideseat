"""Tests for inspector registry and built-in inspectors."""

from __future__ import annotations

from typing import Any

import pytest

from sideseat.runtime.adapters import (
    build_agent_manifest,
    build_mcp_manifest,
    register_agent_inspector,
    register_mcp_inspector,
)
from sideseat.runtime.protocol import RegistrationManifest


class _StubToolRegistry:
    def __init__(self, tools: dict[str, Any]) -> None:
        self._tools = tools

    def get_all_tools_config(self) -> dict[str, Any]:
        return self._tools


class _StubStrandsAgent:
    """Mimics the surface of strands.Agent enough to exercise the inspector."""

    __module__ = "strands.agent.agent"

    def __init__(self) -> None:
        self.tool_registry = _StubToolRegistry({"add": {"name": "add"}, "sub": {"name": "sub"}})
        self.system_prompt = "you are helpful"
        self.model = "stub-model"


_StubStrandsAgent.__name__ = "Agent"


class _StubMcpClient:
    def __init__(self) -> None:
        self._closed = False

    def list_tools_sync(self) -> list[dict[str, Any]]:
        return [{"name": "search", "schema": {"type": "object"}}]


def test_strands_inspector_pulls_tools_from_registry() -> None:
    manifest = build_agent_manifest(_StubStrandsAgent(), name="weather")
    assert isinstance(manifest, RegistrationManifest)
    assert manifest.name == "weather"
    assert manifest.runtime == {"kind": "inproc"}
    assert len(manifest.tools) == 2
    assert manifest.system_prompt == "you are helpful"


def test_strands_inspector_explicit_tools_take_precedence() -> None:
    custom = [{"name": "noop"}]
    manifest = build_agent_manifest(_StubStrandsAgent(), name="weather", tools=custom)
    assert manifest.tools == custom


def test_mcp_inspector_uses_list_tools_sync() -> None:
    manifest = build_mcp_manifest(_StubMcpClient(), name="mcp1", transport="stdio")
    assert manifest.framework == "mcp"
    assert manifest.runtime == {"kind": "mcp", "transport": "stdio"}
    assert manifest.tools == [{"name": "search", "schema": {"type": "object"}}]


def test_unknown_object_without_tools_raises() -> None:
    class _Foreign:
        pass

    with pytest.raises(ValueError, match="no agent inspector matched"):
        build_agent_manifest(_Foreign(), name="x")


def test_register_custom_agent_inspector() -> None:
    class _Foreign:
        pass

    def matcher(obj: Any) -> bool:
        return isinstance(obj, _Foreign)

    def fn(obj: Any, **kwargs: Any) -> RegistrationManifest:
        return RegistrationManifest(
            name=kwargs["name"], framework="custom", runtime=kwargs["runtime"]
        )

    register_agent_inspector(matcher, fn)
    manifest = build_agent_manifest(_Foreign(), name="x")
    assert manifest.framework == "custom"


def test_register_custom_mcp_inspector_when_no_match() -> None:
    class _Foreign:
        pass

    def matcher(obj: Any) -> bool:
        return isinstance(obj, _Foreign)

    def fn(obj: Any, **kwargs: Any) -> RegistrationManifest:
        return RegistrationManifest(name=kwargs["name"], framework="mcp")

    register_mcp_inspector(matcher, fn)
    manifest = build_mcp_manifest(_Foreign(), name="m")
    assert manifest.name == "m"


def test_classify_resolves_agent_and_mcp() -> None:
    from sideseat.runtime.adapters import classify

    assert classify(_StubStrandsAgent()) == "agent"
    assert classify(_StubMcpClient()) == "mcp"
    assert classify(object()) is None


def test_classify_swarm_and_graph_take_precedence_over_agent() -> None:
    from sideseat.runtime.adapters import classify

    class _StubSwarm:
        __module__ = "strands.multiagent.swarm"
        name = "swarm-1"

        def __init__(self) -> None:
            self.nodes = {}

    _StubSwarm.__name__ = "Swarm"
    assert classify(_StubSwarm()) == "swarm"

    class _StubGraph:
        __module__ = "strands.multiagent.graph"
        name = "graph-1"

        def __init__(self) -> None:
            self.nodes = {}

    _StubGraph.__name__ = "Graph"
    assert classify(_StubGraph()) == "graph"


def test_strands_swarm_inspector_lists_inner_nodes() -> None:
    from sideseat.runtime.adapters import build_manifest_for_kind

    class _Node:
        def __init__(self, node_id: str, executor: Any) -> None:
            self.node_id = node_id
            self.executor = executor

    class _StubSwarm:
        __module__ = "strands.multiagent.swarm"

        def __init__(self) -> None:
            self.nodes = {
                "alice": _Node("alice", _StubStrandsAgent()),
                "bob": _Node("bob", _StubStrandsAgent()),
            }

    _StubSwarm.__name__ = "Swarm"

    manifest = build_manifest_for_kind("swarm", _StubSwarm(), name="research")
    assert manifest.framework == "strands-python"
    assert manifest.metadata == {"node_count": 2}
    assert {n["node_id"] for n in manifest.tools} == {"alice", "bob"}
    assert all(n["type"] == "Agent" for n in manifest.tools)


def test_derive_default_name_uses_obj_name() -> None:
    from sideseat.runtime.adapters import derive_default_name

    class _Named:
        name = "calc"

    class _Anonymous:
        pass

    assert derive_default_name(_Named(), "agent") == "calc"
    assert derive_default_name(_Anonymous(), "agent") is None


def test_agentcore_endpoint_lifted_into_runtime() -> None:
    manifest = build_agent_manifest(
        _StubStrandsAgent(),
        name="ac",
        runtime="agentcore_local",
        agentcore_endpoint="ws://127.0.0.1:8080",
    )
    assert manifest.runtime == {
        "kind": "agentcore_local",
        "endpoint": "ws://127.0.0.1:8080",
    }
