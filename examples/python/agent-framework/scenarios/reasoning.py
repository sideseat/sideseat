from agent_framework import Agent
from models import REASONING_OPTIONS

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(client=run.llm, name="assistant", default_options=REASONING_OPTIONS)
    session = agent.create_session(session_id=run.session_id)
    with run.trace():
        print((await agent.run(content.REASONING, session=session)).text)
