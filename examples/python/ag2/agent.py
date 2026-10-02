"""The agent the scenarios run, with AG2's telemetry middleware in native mode."""

from collections.abc import Iterable
from typing import Any

from ag2 import Agent
from ag2.config import ModelConfig
from ag2.middleware.builtin import TelemetryMiddleware
from opentelemetry import trace

from harness import Run, content


def build_agent(
    run: Run,
    name: str = "assistant",
    *,
    prompt: str | None = content.SYSTEM,
    config: ModelConfig | None = None,
    tools: Iterable[Any] = (),
) -> Agent[Any]:
    # AG2 documents tracing as middleware on each agent. In SDK mode sideseat.init attaches the
    # same middleware to every agent itself.
    middleware = (
        [
            TelemetryMiddleware(
                tracer_provider=trace.get_tracer_provider(),  # type: ignore[arg-type]
                capture_content=True,
                agent_name=name,
            )
        ]
        if run.mode == "native"
        else []
    )
    return Agent(
        name,
        prompt if prompt is not None else (),
        config=config or run.llm,
        tools=tools,
        middleware=middleware,
    )
