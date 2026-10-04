from agno.agent import Agent
from results import answer

from harness import Run, content


def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        agent = Agent(model=run.llm, instructions=content.SYSTEM)
        with run.trace(f"session-turn-{index}"):
            result = agent.run(question, session_id=run.session_id, user_id=run.user_id)
            print(answer(result))
