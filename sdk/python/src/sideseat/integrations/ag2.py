"""AG2: adds its telemetry middleware to every agent created after init."""

from __future__ import annotations

import functools
import importlib
from typing import Any

from sideseat.integrations._base import Integration, SetupContext


class AG2(Integration):
    name = "ag2"
    packages = ("ag2",)
    extra = "ag2"

    def __init__(self) -> None:
        self._original_init: Any = None

    def instrument(self, ctx: SetupContext) -> None:
        agent_cls = importlib.import_module("ag2").Agent
        telemetry_cls = importlib.import_module("ag2.middleware.builtin").TelemetryMiddleware
        original = agent_cls.__init__
        provider = ctx.tracer_provider
        capture_content = ctx.capture_content

        @functools.wraps(original)
        def init_with_telemetry(agent: Any, *args: Any, **kwargs: Any) -> None:
            middleware = tuple(kwargs.pop("middleware", ()))
            if not any(isinstance(item, telemetry_cls) for item in middleware):
                name = args[0] if args else kwargs.get("name", "agent")
                middleware = (
                    *middleware,
                    telemetry_cls(
                        tracer_provider=provider,
                        capture_content=capture_content,
                        agent_name=str(name),
                    ),
                )
            original(agent, *args, middleware=middleware, **kwargs)

        agent_cls.__init__ = init_with_telemetry
        self._original_init = original

    def shutdown(self) -> None:
        if self._original_init is not None:
            importlib.import_module("ag2").Agent.__init__ = self._original_init
            self._original_init = None
