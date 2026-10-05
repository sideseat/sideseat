from agents import Agent, Runner
from tools import get_precipitation, get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(
        name="assistant",
        instructions=content.SYSTEM,
        model=run.llm,
        tools=[get_weather, get_precipitation],
    )
    with run.trace():
        result = await Runner.run(agent, content.TOOL_USE)
        print(result.final_output)
