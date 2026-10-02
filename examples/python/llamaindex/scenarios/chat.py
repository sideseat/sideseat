from agent import build_agent

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run.llm)
    with run.trace():
        print(await agent.run(user_msg=content.CHAT))
