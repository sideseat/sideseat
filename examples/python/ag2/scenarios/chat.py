from agent import build_agent

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run)
    with run.trace():
        print(await (await agent.ask(content.CHAT)).content())
