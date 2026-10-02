from agent import build_agent
from tools import book_flight

from harness import Run, content


async def run(run: Run) -> None:
    # AG2 returns the tool's exception to the model as an error result.
    agent = build_agent(run, tools=[book_flight])
    with run.trace():
        print(await (await agent.ask(content.ERROR)).content())
