from pydantic_ai import Agent

from harness import Run, content


def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    agent = Agent(run.llm, instructions=content.SYSTEM)
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            print(agent.run_sync(question).output)
