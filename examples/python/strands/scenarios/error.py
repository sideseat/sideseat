from strands import Agent
from tools import book_flight

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(
        model=run.llm,
        system_prompt=content.SYSTEM,
        tools=[book_flight],
        callback_handler=None,
    )
    with run.trace():
        print(agent(content.ERROR))
