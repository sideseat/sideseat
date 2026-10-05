from contextlib import AbstractContextManager, nullcontext

from pydantic_ai import Agent
from tools import get_precipitation, get_weather

from harness import Run, content


def sequential_tools() -> AbstractContextManager[object]:
    """Run tools one at a time, where the release can (the version matrix replays older ones).

    Concurrent tools finish in a different order on every run, and each result is placed when its tool
    finished; running them one at a time keeps the native and SDK captures comparable. The switch is
    public from 1.107, private (`_tool_manager`) before, and absent in the earliest 1.x releases.
    """
    for module in ("pydantic_ai.tool_manager", "pydantic_ai._tool_manager"):
        try:
            manager = __import__(module, fromlist=["ToolManager"]).ToolManager
        except (ImportError, AttributeError):
            continue
        if hasattr(manager, "parallel_execution_mode"):
            return manager.parallel_execution_mode("sequential")
    return nullcontext()


def run(run: Run) -> None:
    agent = Agent(
        run.llm, instructions=content.SYSTEM, tools=[get_weather, get_precipitation]
    )
    with run.trace(), sequential_tools():
        print(agent.run_sync(content.TOOL_USE).output)
