from smolagents import ToolCallingAgent
from tools import get_precipitation, get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = ToolCallingAgent(
        tools=[get_weather, get_precipitation],
        model=run.llm,
        instructions=content.SYSTEM,
        # Parallel calls finish in a different order on every run, and their results carry no call id, so
        # SideSeat pairs each with the oldest unanswered call of its name; one thread keeps them in order.
        max_tool_threads=1,
    )
    with run.trace():
        print(agent.run(content.TOOL_USE))
