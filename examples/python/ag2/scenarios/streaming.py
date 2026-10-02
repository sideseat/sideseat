from agent import build_agent
from models import build
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(
        run, config=build(run.model, streaming=True), tools=[get_weather]
    )
    with run.trace():
        print(await (await agent.ask(content.STREAMING)).content())
