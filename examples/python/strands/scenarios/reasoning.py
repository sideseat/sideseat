from models import build
from strands import Agent

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(model=build(run.model, reasoning=True), callback_handler=None)
    with run.trace():
        print(agent(content.REASONING))
