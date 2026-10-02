from agent_framework import Agent

from harness import Run, content


async def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    agent = Agent(client=run.llm, name="assistant", instructions=content.SYSTEM)
    for index, question in enumerate(content.SESSION, start=1):
        session = agent.create_session(session_id=run.session_id)
        with run.trace(f"session-turn-{index}"):
            print((await agent.run(question, session=session)).text)
