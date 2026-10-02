from agent import build_agent
from tools import get_precipitation, get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run.llm, tools=[get_weather, get_precipitation])
    with run.trace():
        print(await agent.run(user_msg=content.TOOL_USE))
