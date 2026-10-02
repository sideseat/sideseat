from smolagents import ToolCallingAgent
from tools import book_flight

from harness import Run, content


def run(run: Run) -> None:
    agent = ToolCallingAgent(
        tools=[book_flight], model=run.llm, instructions=content.SYSTEM
    )
    with run.trace():
        print(agent.run(content.ERROR))
