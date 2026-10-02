from agent_framework import Agent

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(client=run.llm, name="assistant", instructions=content.SYSTEM)
    # The session keeps the conversation, so each request re-sends the earlier turns.
    session = agent.create_session(session_id=run.session_id)
    with run.trace():
        for question in content.MULTI_TURN:
            print((await agent.run(question, session=session)).text)
