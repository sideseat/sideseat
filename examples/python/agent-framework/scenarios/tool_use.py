from agent_framework import Agent
from tools import get_precipitation, get_weather

from harness import Run, content


async def run(run: Run) -> None:
    # Concurrent tools finish in a different order on every run, and each result is placed when its tool
    # finished; running them in model order keeps the native and SDK captures comparable.
    run.llm.function_invocation_configuration["allow_concurrent_invocation"] = False
    agent = Agent(
        client=run.llm,
        name="assistant",
        instructions=content.SYSTEM,
        tools=[get_weather, get_precipitation],
    )
    session = agent.create_session(session_id=run.session_id)
    with run.trace():
        print((await agent.run(content.TOOL_USE, session=session)).text)
