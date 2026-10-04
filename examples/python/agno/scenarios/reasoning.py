from agno.agent import Agent
from models import build
from results import answer

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(model=build(run.model, reasoning=True))
    with run.trace():
        result = agent.run(
            content.REASONING, session_id=run.session_id, user_id=run.user_id
        )
        print(answer(result))
