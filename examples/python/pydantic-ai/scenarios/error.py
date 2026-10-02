from pydantic_ai import Agent
from tools import book_flight

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(run.llm, instructions=content.SYSTEM, tools=[book_flight], retries=2)
    with run.trace():
        print(agent.run_sync(content.ERROR).output)
