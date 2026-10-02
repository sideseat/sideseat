from strands import Agent
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(
        model=run.llm,
        system_prompt=content.SYSTEM,
        tools=[get_weather],
        callback_handler=None,
    )
    with run.trace():
        async for event in agent.stream_async(content.STREAMING):
            if "data" in event:
                print(event["data"], end="", flush=True)
        print()
