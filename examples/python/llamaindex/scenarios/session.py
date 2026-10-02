from agent import build_agent

from harness import Run, content


async def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    agent = build_agent(run.llm)
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            print(await agent.run(user_msg=question))
