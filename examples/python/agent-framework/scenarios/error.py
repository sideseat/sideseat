from agent_framework import Agent
from tools import book_flight

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(
        client=run.llm,
        name="assistant",
        instructions=content.SYSTEM,
        tools=[book_flight],
    )
    session = agent.create_session(session_id=run.session_id)
    with run.trace():
        print((await agent.run(content.ERROR, session=session)).text)
