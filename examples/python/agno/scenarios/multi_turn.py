from agno.agent import Agent
from agno.db.in_memory import InMemoryDb
from results import answer

from harness import Run, content


def run(run: Run) -> None:
    # Agno keeps conversation history in a session store and re-sends it when asked to.
    agent = Agent(
        model=run.llm,
        instructions=content.SYSTEM,
        db=InMemoryDb(),
        add_history_to_context=True,
    )
    with run.trace():
        for question in content.MULTI_TURN:
            result = agent.run(question, session_id=run.session_id, user_id=run.user_id)
            print(answer(result))
