"""History, streaming, tools, traces, and sessions in one AgentScope capture."""

from collections.abc import Callable
from typing import Any

from agentscope.agent import Agent, InjectionConfig
from agentscope.message import UserMsg
from agentscope.middleware import TracingMiddleware
from agentscope.permission import PermissionBehavior, PermissionDecision
from agentscope.state import AgentState
from agentscope.tool import FunctionTool, Toolkit

_weather_calls = 0


def get_weather(location: str) -> str:
    """Get deterministic weather for a city."""
    global _weather_calls
    _weather_calls += 1
    return f"Sunny, 22°C, light breeze in {location}."


def _middlewares(use_sideseat: bool) -> list[Any]:
    """Return native tracing; SideSeat injects the SDK counterpart."""
    return [] if use_sideseat else [TracingMiddleware()]


def _agent(
    *,
    name: str,
    system_prompt: str,
    model: Any,
    session_id: str,
    use_sideseat: bool,
    toolkit: Toolkit | None = None,
) -> Agent:
    """Create an AgentScope agent without time-dependent prompt injection."""
    return Agent(
        name=name,
        system_prompt=system_prompt,
        model=model,
        toolkit=toolkit,
        middlewares=_middlewares(use_sideseat),
        state=AgentState(session_id=session_id),
        injection_config=InjectionConfig(inject_runtime_state=False),
    )


def _text(message: Any, description: str) -> str:
    """Return one non-empty AgentScope response body."""
    value = message.get_text_content()
    text = str(value or "").strip()
    if not text:
        raise RuntimeError(f"AgentScope {description} response was empty")
    return text


async def run(
    model_factory: Callable[..., Any],
    trace_attrs: dict[str, str],
    client: Any,
    use_sideseat: bool,
) -> None:
    """Emit two traces that share one session and cover the production rubric."""
    global _weather_calls
    _weather_calls = 0
    session_id = trace_attrs["session.id"]
    user_id = trace_attrs["user.id"]

    history_agent = _agent(
        name="HistoryAgent",
        system_prompt="Answer scientific questions in one sentence.",
        model=model_factory(),
        session_id=session_id,
        use_sideseat=use_sideseat,
    )
    with client.trace(
        "agentscope-history",
        session_id=session_id,
        user_id=user_id,
    ):
        first = await history_agent.reply(
            UserMsg("user", "What is the speed of light?"),
        )
        _text(first, "non-streaming")

        history_agent.model.stream = True
        second = await history_agent.reply(
            UserMsg("user", "What is the boiling point of water?"),
        )
        print(_text(second, "streaming"))

    weather_tool = FunctionTool(
        get_weather,
        permission=PermissionDecision(
            PermissionBehavior.ALLOW,
            "The deterministic fixture tool is safe.",
        ),
    )
    tool_agent = _agent(
        name="ToolAgent",
        system_prompt="Use tools when they are available.",
        model=model_factory(),
        toolkit=Toolkit(tools=[weather_tool]),
        session_id=session_id,
        use_sideseat=use_sideseat,
    )
    with client.trace(
        "agentscope-tool-roundtrip",
        session_id=session_id,
        user_id=user_id,
    ):
        response = await tool_agent.reply(
            UserMsg("user", "What is the weather in Paris?"),
        )
        print(_text(response, "tool"))

    if _weather_calls != 1:
        raise RuntimeError(f"AgentScope executed get_weather {_weather_calls} times")
