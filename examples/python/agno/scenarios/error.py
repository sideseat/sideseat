from agno.agent import Agent
from tools import book_flight
from results import answer

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(model=run.llm, instructions=content.SYSTEM, tools=[book_flight])
    with run.trace():
        result = agent.run(
            content.ERROR, session_id=run.session_id, user_id=run.user_id
        )
        print(answer(result))
