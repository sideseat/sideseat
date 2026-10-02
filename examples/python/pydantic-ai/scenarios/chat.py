from pydantic_ai import Agent

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(run.llm, instructions=content.SYSTEM)
    with run.trace():
        print(agent.run_sync(content.CHAT).output)
