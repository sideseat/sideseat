from agent import answer, assistant
from tools import get_precipitation, get_weather

from harness import Run, content


async def run(run: Run) -> None:
    with run.trace():
        agent = assistant(run.llm, tools=[get_weather, get_precipitation])
        print(answer(await agent.run(task=content.TOOL_USE)))
