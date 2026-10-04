from pydantic_ai import Agent
from pydantic_ai.tool_manager import ToolManager
from tools import get_precipitation, get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(
        run.llm, instructions=content.SYSTEM, tools=[get_weather, get_precipitation]
    )
    # Concurrent tools finish in a different order on every run, and each result is placed when its
    # tool finished; running them one at a time keeps the native and SDK captures comparable.
    with run.trace(), ToolManager.parallel_execution_mode("sequential"):
        print(agent.run_sync(content.TOOL_USE).output)
