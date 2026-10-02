from strands import Agent
from tools import get_precipitation, get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(
        model=run.llm,
        system_prompt=content.SYSTEM,
        tools=[get_weather, get_precipitation],
        callback_handler=None,
    )
    with run.trace():
        print(agent(content.TOOL_USE))
