from agent_framework import Agent
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(
        client=run.llm,
        name="assistant",
        instructions=content.SYSTEM,
        tools=[get_weather],
    )
    session = agent.create_session(session_id=run.session_id)
    with run.trace():
        async for update in agent.run(content.STREAMING, stream=True, session=session):
            print(update.text, end="", flush=True)
        print()
