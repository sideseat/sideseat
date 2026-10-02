"""AgentScope: adds its tracing middleware to every agent created after init."""

from __future__ import annotations

import functools
import importlib
import inspect
from typing import Any

from sideseat.integrations._base import Integration, SetupContext


class AgentScope(Integration):
    name = "agentscope"
    packages = ("agentscope",)
    extra = "agentscope"

    def __init__(self) -> None:
        self._original_init: Any = None

    def instrument(self, ctx: SetupContext) -> None:
        agent_cls = importlib.import_module("agentscope.agent").Agent
        tracing_cls = importlib.import_module("agentscope.middleware").TracingMiddleware
        original = agent_cls.__init__
        signature = inspect.signature(original)

        @functools.wraps(original)
        def init_with_tracing(agent: Any, *args: Any, **kwargs: Any) -> None:
            # Bind through the real signature so positional and keyword callers both keep working.
            bound = signature.bind(agent, *args, **kwargs)
            middlewares = list(bound.arguments.get("middlewares") or ())
            if not any(isinstance(item, tracing_cls) for item in middlewares):
                middlewares.append(tracing_cls())
            bound.arguments["middlewares"] = middlewares
            original(*bound.args, **bound.kwargs)

        agent_cls.__init__ = init_with_tracing
        self._original_init = original

    def shutdown(self) -> None:
        if self._original_init is not None:
            importlib.import_module("agentscope.agent").Agent.__init__ = self._original_init
            self._original_init = None
