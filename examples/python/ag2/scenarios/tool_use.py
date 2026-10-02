from agent import build_agent
from tools import get_precipitation, get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run, tools=[get_weather, get_precipitation])
    with run.trace():
        print(await (await agent.ask(content.TOOL_USE)).content())
