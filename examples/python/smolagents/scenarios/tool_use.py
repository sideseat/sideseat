from smolagents import ToolCallingAgent
from tools import get_precipitation, get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = ToolCallingAgent(
        tools=[get_weather, get_precipitation],
        model=run.llm,
        instructions=content.SYSTEM,
    )
    with run.trace():
        print(agent.run(content.TOOL_USE))
