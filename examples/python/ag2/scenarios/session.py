from agent import build_agent

from harness import Run, content


async def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        agent = build_agent(run)
        with run.trace(f"session-turn-{index}"):
            print(await (await agent.ask(question)).content())
