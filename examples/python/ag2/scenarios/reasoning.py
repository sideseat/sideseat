from agent import build_agent
from models import build

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run, prompt=None, config=build(run.model, reasoning=True))
    with run.trace():
        print(await (await agent.ask(content.REASONING)).content())
