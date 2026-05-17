"""Pluggable inspector registry for converting live runtime objects into
`RegistrationManifest` payloads.

Built-in inspectors handle Strands `Agent` and any object with a
`list_tools_sync()` method (including `strands_tools.MCPClient`).
External users can register their own inspectors via `register_*_inspector`.
"""

from __future__ import annotations

import logging
import threading
from collections import OrderedDict
from collections.abc import Callable
from typing import Any

from sideseat.runtime.protocol import RegistrationManifest

logger = logging.getLogger("sideseat.runtime.adapters")

InspectorMatcher = Callable[[Any], bool]
InspectorFn = Callable[..., RegistrationManifest]


class _InspectorRegistry:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._inspectors: dict[str, list[tuple[InspectorMatcher, InspectorFn]]] = {
            "agent": [],
            "mcp": [],
            "swarm": [],
            "graph": [],
        }
        self._warn_lru: OrderedDict[tuple[str, str], None] = OrderedDict()
        self._warn_lru_capacity = 256

    def register(self, kind: str, matcher: InspectorMatcher, fn: InspectorFn) -> None:
        with self._lock:
            self._inspectors.setdefault(kind, []).append((matcher, fn))

    def find(self, kind: str, obj: Any) -> InspectorFn | None:
        with self._lock:
            inspectors = list(self._inspectors.get(kind, ()))
        for m, fn in reversed(inspectors):
            if m(obj):
                return fn
        return None

    def warn_once(self, kind: str, name: str, msg: str) -> None:
        key = (kind, name)
        with self._lock:
            if key in self._warn_lru:
                return
            self._warn_lru[key] = None
            if len(self._warn_lru) > self._warn_lru_capacity:
                self._warn_lru.popitem(last=False)
        logger.warning("inspector for %s/%s: %s", kind, name, msg)


_REGISTRY = _InspectorRegistry()


def classify(obj: Any) -> str | None:
    """Return 'swarm' | 'graph' | 'agent' | 'mcp' | None for `obj` based on the
    registered inspectors. Composite kinds (swarm/graph) are checked first so a
    Swarm built out of Agents is not misclassified as a single Agent."""
    for kind in KINDS:
        if _REGISTRY.find(kind, obj) is not None:
            return kind
    return None


def find_inspector(kind: str, obj: Any) -> InspectorFn | None:
    """Public lookup used by RuntimeClient.register()."""
    return _REGISTRY.find(kind, obj)


def derive_default_name(obj: Any, kind: str) -> str | None:
    """Best-effort: return a name to use when the caller didn't supply one."""
    name = getattr(obj, "name", None)
    if isinstance(name, str) and name:
        return name
    return None


def register_agent_inspector(matcher: InspectorMatcher, fn: InspectorFn) -> None:
    _REGISTRY.register("agent", matcher, fn)


def register_mcp_inspector(matcher: InspectorMatcher, fn: InspectorFn) -> None:
    _REGISTRY.register("mcp", matcher, fn)


def register_swarm_inspector(matcher: InspectorMatcher, fn: InspectorFn) -> None:
    _REGISTRY.register("swarm", matcher, fn)


def register_graph_inspector(matcher: InspectorMatcher, fn: InspectorFn) -> None:
    _REGISTRY.register("graph", matcher, fn)


KINDS = ("swarm", "graph", "agent", "mcp")  # order matters: composite first


# ---------------------------------------------------------------------------
# Built-in inspectors
# ---------------------------------------------------------------------------


STRANDS_FRAMEWORK = "strands-python"


def _looks_like_strands_agent(obj: Any) -> bool:
    cls = type(obj)
    if not getattr(cls, "__module__", "").startswith("strands"):
        return False
    return getattr(cls, "__name__", "") == "Agent"


def _looks_like_strands_swarm(obj: Any) -> bool:
    cls = type(obj)
    if not getattr(cls, "__module__", "").startswith("strands"):
        return False
    return getattr(cls, "__name__", "") == "Swarm"


def _looks_like_strands_graph(obj: Any) -> bool:
    cls = type(obj)
    if not getattr(cls, "__module__", "").startswith("strands"):
        return False
    return getattr(cls, "__name__", "") == "Graph"


def inspect_strands_agent(
    obj: Any,
    *,
    name: str,
    framework: str = STRANDS_FRAMEWORK,
    runtime: dict[str, Any] | None = None,
    model: str | None = None,
    system_prompt: str | None = None,
    tools: list[Any] | None = None,
    metadata: dict[str, Any] | None = None,
) -> RegistrationManifest:
    discovered_tools: list[Any] = list(tools or [])
    if not discovered_tools:
        try:
            registry = getattr(obj, "tool_registry", None)
            if registry is not None and hasattr(registry, "get_all_tools_config"):
                config = registry.get_all_tools_config()  # type: ignore[attr-defined]
                if isinstance(config, dict):
                    discovered_tools = list(config.values())
                elif isinstance(config, list):
                    discovered_tools = config
        except Exception as exc:  # pragma: no cover - inspector best-effort
            _REGISTRY.warn_once(
                "agent",
                name,
                f"strands tool introspection raised {type(exc).__name__}: {exc}",
            )

    inferred_model = model or getattr(obj, "model_id", None) or _stringify(getattr(obj, "model", None))
    inferred_prompt = system_prompt or getattr(obj, "system_prompt", None)

    return RegistrationManifest(
        name=name,
        framework=framework,
        runtime=runtime or {"kind": "inproc"},
        model=inferred_model if isinstance(inferred_model, str) else None,
        system_prompt=inferred_prompt if isinstance(inferred_prompt, str) else None,
        tools=discovered_tools,
        metadata=metadata or {},
    )


def _strands_node_summary(executor: Any) -> dict[str, Any]:
    """Summarise a Swarm/Graph node executor for the manifest."""
    cls_name = type(executor).__name__
    name = getattr(executor, "name", None)
    summary: dict[str, Any] = {"type": cls_name}
    if isinstance(name, str):
        summary["name"] = name
    if cls_name == "Agent":
        try:
            registry = getattr(executor, "tool_registry", None)
            if registry is not None and hasattr(registry, "get_all_tools_config"):
                cfg = registry.get_all_tools_config()
                summary["tool_names"] = sorted(cfg.keys()) if isinstance(cfg, dict) else None
        except Exception:
            pass
    return summary


def _swarm_or_graph_nodes(obj: Any) -> list[dict[str, Any]]:
    nodes_attr = getattr(obj, "nodes", None)
    if not isinstance(nodes_attr, dict):
        return []
    out: list[dict[str, Any]] = []
    for node_id, node in nodes_attr.items():
        executor = getattr(node, "executor", None)
        item: dict[str, Any] = {"node_id": str(node_id)}
        if executor is not None:
            item.update(_strands_node_summary(executor))
        deps = getattr(node, "dependencies", None)
        if deps is not None:
            try:
                item["dependencies"] = sorted(getattr(d, "node_id", str(d)) for d in deps)
            except Exception:
                pass
        out.append(item)
    return out


def inspect_strands_swarm(
    obj: Any,
    *,
    name: str,
    framework: str = STRANDS_FRAMEWORK,
    runtime: dict[str, Any] | None = None,
    metadata: dict[str, Any] | None = None,
    **_unused: Any,
) -> RegistrationManifest:
    nodes = _swarm_or_graph_nodes(obj)
    return RegistrationManifest(
        name=name,
        framework=framework,
        runtime=runtime or {"kind": "inproc"},
        tools=nodes,  # opaque on the server; carries node-level summary
        metadata={**(metadata or {}), "node_count": len(nodes)},
    )


def inspect_strands_graph(
    obj: Any,
    *,
    name: str,
    framework: str = STRANDS_FRAMEWORK,
    runtime: dict[str, Any] | None = None,
    metadata: dict[str, Any] | None = None,
    **_unused: Any,
) -> RegistrationManifest:
    nodes = _swarm_or_graph_nodes(obj)
    return RegistrationManifest(
        name=name,
        framework=framework,
        runtime=runtime or {"kind": "inproc"},
        tools=nodes,
        metadata={**(metadata or {}), "node_count": len(nodes)},
    )


def _looks_like_mcp_client(obj: Any) -> bool:
    return callable(getattr(obj, "list_tools_sync", None))


def inspect_mcp_client(
    obj: Any,
    *,
    name: str,
    transport: str | None = None,
    url: str | None = None,
    tools: list[Any] | None = None,
    metadata: dict[str, Any] | None = None,
) -> RegistrationManifest:
    discovered_tools: list[Any] = list(tools or [])
    if not discovered_tools:
        try:
            list_fn = getattr(obj, "list_tools_sync", None)
            if callable(list_fn):
                result = list_fn()
                if hasattr(result, "tools"):
                    result = list(result.tools)  # type: ignore[attr-defined]
                if isinstance(result, list):
                    discovered_tools = [_to_jsonable(t) for t in result]
        except Exception as exc:
            _REGISTRY.warn_once(
                "mcp",
                name,
                f"mcp list_tools_sync raised {type(exc).__name__}: {exc}. "
                "Note: list_tools_sync only works inside the client's context manager.",
            )

    runtime: dict[str, Any] = {"kind": "mcp"}
    if transport:
        runtime["transport"] = transport
    if url:
        runtime["url"] = url

    return RegistrationManifest(
        name=name,
        framework="mcp",
        runtime=runtime,
        tools=discovered_tools,
        metadata=metadata or {},
    )


def _stringify(value: Any) -> str | None:
    if value is None:
        return None
    if isinstance(value, str):
        return value
    return str(value)


def _to_jsonable(value: Any) -> Any:
    if hasattr(value, "model_dump"):
        try:
            return value.model_dump()  # type: ignore[no-any-return]
        except Exception:
            pass
    if hasattr(value, "__dict__"):
        return {k: _to_jsonable(v) for k, v in vars(value).items() if not k.startswith("_")}
    if isinstance(value, list):
        return [_to_jsonable(v) for v in value]
    if isinstance(value, dict):
        return {k: _to_jsonable(v) for k, v in value.items()}
    return value


# Built-in inspectors are registered eagerly so consumers do not need to
# import this module before calling the entrypoints.
register_agent_inspector(_looks_like_strands_agent, inspect_strands_agent)
register_mcp_inspector(_looks_like_mcp_client, inspect_mcp_client)
register_swarm_inspector(_looks_like_strands_swarm, inspect_strands_swarm)
register_graph_inspector(_looks_like_strands_graph, inspect_strands_graph)


def build_agent_manifest(
    instance: Any,
    *,
    name: str,
    runtime: str | dict[str, Any] = "inproc",
    agentcore_endpoint: str | None = None,
    tools: list[Any] | None = None,
    system_prompt: str | None = None,
    model: str | None = None,
    metadata: dict[str, Any] | None = None,
) -> RegistrationManifest:
    runtime_descriptor = _normalize_runtime(runtime, agentcore_endpoint)
    fn = _REGISTRY.find("agent", instance)
    if fn is None:
        if not tools:
            raise ValueError(
                f"no agent inspector matched {type(instance).__name__!r}; "
                "register one via sideseat.runtime.adapters.register_agent_inspector(...) "
                "or pass tools=[...]"
            )
        return RegistrationManifest(
            name=name,
            runtime=runtime_descriptor,
            model=model,
            system_prompt=system_prompt,
            tools=list(tools),
            metadata=metadata or {},
        )
    manifest = fn(
        instance,
        name=name,
        runtime=runtime_descriptor,
        tools=tools,
        system_prompt=system_prompt,
        model=model,
        metadata=metadata,
    )
    return manifest


def build_mcp_manifest(
    client: Any,
    *,
    name: str,
    transport: str | None = None,
    url: str | None = None,
    tools: list[Any] | None = None,
    metadata: dict[str, Any] | None = None,
) -> RegistrationManifest:
    fn = _REGISTRY.find("mcp", client)
    if fn is None:
        if not tools:
            raise ValueError(
                f"no MCP inspector matched {type(client).__name__!r}; "
                "register one via sideseat.runtime.adapters.register_mcp_inspector(...) "
                "or pass tools=[...]"
            )
        runtime: dict[str, Any] = {"kind": "mcp"}
        if transport:
            runtime["transport"] = transport
        if url:
            runtime["url"] = url
        return RegistrationManifest(
            name=name,
            framework="mcp",
            runtime=runtime,
            tools=list(tools),
            metadata=metadata or {},
        )
    return fn(
        client,
        name=name,
        transport=transport,
        url=url,
        tools=tools,
        metadata=metadata,
    )


def build_manifest_for_kind(
    kind: str,
    obj: Any,
    *,
    name: str,
    runtime: str | dict[str, Any] = "inproc",
    agentcore_endpoint: str | None = None,
    metadata: dict[str, Any] | None = None,
) -> RegistrationManifest:
    """Generic dispatcher used by RuntimeClient.register()."""
    fn = _REGISTRY.find(kind, obj)
    if fn is None:
        raise ValueError(
            f"no {kind} inspector matched {type(obj).__name__!r}; register one via "
            f"sideseat.runtime.adapters.register_{kind}_inspector(...)"
        )
    runtime_descriptor = _normalize_runtime(runtime, agentcore_endpoint)
    return fn(
        obj,
        name=name,
        runtime=runtime_descriptor,
        metadata=metadata,
    )


def _normalize_runtime(
    runtime: str | dict[str, Any],
    agentcore_endpoint: str | None,
) -> dict[str, Any]:
    if isinstance(runtime, dict):
        out = dict(runtime)
    else:
        out = {"kind": runtime}
    if agentcore_endpoint and "endpoint" not in out:
        out["endpoint"] = agentcore_endpoint
    return out
