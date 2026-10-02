from agent_framework import Agent
from tools import get_precipitation, get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(
        client=run.llm,
        name="assistant",
        instructions=content.SYSTEM,
        tools=[get_weather, get_precipitation],
    )
    session = agent.create_session(session_id=run.session_id)
    with run.trace():
        print((await agent.run(content.TOOL_USE, session=session)).text)
