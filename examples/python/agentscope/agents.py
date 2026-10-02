"""How every scenario builds an AgentScope agent."""

from typing import Any

from agentscope.agent import Agent, InjectionConfig
from agentscope.middleware import MiddlewareBase, TracingMiddleware
from agentscope.permission import PermissionContext, PermissionMode
from agentscope.state import AgentState
from agentscope.tool import Toolkit

from harness import Run, content


def agent(
    run: Run,
    *,
    name: str = "assistant",
    system_prompt: str = content.SYSTEM,
    model: Any = None,
    toolkit: Toolkit | None = None,
) -> Agent:
    # Native telemetry is AgentScope's TracingMiddleware, passed to each agent as its documentation shows;
    # in SDK mode sideseat.init adds the same middleware to every agent itself.
    middlewares: list[MiddlewareBase] = (
        [TracingMiddleware()] if run.mode == "native" else []
    )
    return Agent(
        name=name,
        system_prompt=system_prompt,
        model=model or run.llm,
        toolkit=toolkit,
        middlewares=middlewares,
        state=AgentState(
            session_id=run.session_id,
            # The scenarios run unattended, so tools run without asking for confirmation.
            permission_context=PermissionContext(mode=PermissionMode.BYPASS),
        ),
        # The injected runtime state carries the current time, which would make every request unique.
        injection_config=InjectionConfig(inject_runtime_state=False),
    )
