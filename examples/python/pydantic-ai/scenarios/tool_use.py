from pydantic_ai import Agent
from tools import get_precipitation, get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(
        run.llm, instructions=content.SYSTEM, tools=[get_weather, get_precipitation]
    )
    with run.trace():
        print(agent.run_sync(content.TOOL_USE).output)
