from pydantic_ai import Agent
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(run.llm, instructions=content.SYSTEM, tools=[get_weather])
    with run.trace():
        async with agent.run_stream(content.STREAMING) as result:
            async for delta in result.stream_text(delta=True):
                print(delta, end="", flush=True)
        print()
