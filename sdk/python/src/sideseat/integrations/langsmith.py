"""LangSmith OTel mode: LangChain and LangGraph runs become spans on SideSeat's provider."""

from __future__ import annotations

import os
from typing import ClassVar

from sideseat.integrations._base import Integration, SetupContext


class LangSmith(Integration):
    name = "langsmith"
    packages = ("langsmith",)
    extra = "langsmith"
    # LangSmith is a dependency of LangChain, so its presence says nothing about how the application
    # traces; it is chosen by name.
    detectable = False

    _SWITCHES: ClassVar[dict[str, str]] = {
        "LANGSMITH_TRACING": "true",
        "LANGSMITH_TRACING_MODE": "otel",
    }

    def prepare(self, ctx: SetupContext) -> None:
        # LangSmith reads these when it first builds its client - lazily, at the first run it
        # traces - so they hold for as long as SDK tracing does, and shutdown restores what was
        # there before. `otel` sends every run to the global tracer provider, SideSeat's, and
        # nothing to LangSmith's API.
        self._previous = {key: os.environ.get(key) for key in self._SWITCHES}
        os.environ.update(self._SWITCHES)

    def flush(self, timeout_millis: int) -> bool:
        # LangSmith hands runs to OpenTelemetry from a background thread; until it has, the provider
        # has nothing to flush. LangChain, LangGraph and `@traceable` report through this client.
        from langsmith.run_trees import get_cached_client

        get_cached_client().flush(timeout=timeout_millis / 1000)
        return True

    def shutdown(self) -> None:
        for key, value in getattr(self, "_previous", {}).items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
