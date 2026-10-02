from agent_framework import Agent

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(client=run.llm, name="assistant", instructions=content.SYSTEM)
    session = agent.create_session(session_id=run.session_id)
    with run.trace():
        response = await agent.run(
            content.STRUCTURED,
            session=session,
            options={"response_format": content.TripPlan},
        )
        print(response.value)
