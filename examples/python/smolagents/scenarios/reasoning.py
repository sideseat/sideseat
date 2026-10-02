from models import build
from smolagents import ToolCallingAgent

from harness import Run, content


def run(run: Run) -> None:
    agent = ToolCallingAgent(tools=[], model=build(run.model, reasoning=True))
    with run.trace():
        print(agent.run(content.REASONING))
