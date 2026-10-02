from models import build
from pydantic_ai import Agent

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(build(run.model, reasoning=True))
    with run.trace():
        print(agent.run_sync(content.REASONING).output)
