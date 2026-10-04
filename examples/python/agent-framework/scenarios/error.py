from agent_framework import Agent
from tools import book_flight

from harness import Run, content


async def run(run: Run) -> None:
    # Agent Framework tells the model only that the function failed unless asked for the details, and the
    # scenario is the model reading the error.
    run.llm.function_invocation_configuration["include_detailed_errors"] = True
    agent = Agent(
        client=run.llm,
        name="assistant",
        instructions=content.SYSTEM,
        tools=[book_flight],
    )
    session = agent.create_session(session_id=run.session_id)
    with run.trace():
        print((await agent.run(content.ERROR, session=session)).text)
