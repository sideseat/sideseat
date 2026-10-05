from agents import Agent, Runner
from tools import book_flight

from harness import Run, content


async def run(run: Run) -> None:
    # A function tool that raises answers the model with the error instead of ending the run.
    agent = Agent(
        name="assistant",
        instructions=content.SYSTEM,
        model=run.llm,
        tools=[book_flight],
    )
    with run.trace():
        result = await Runner.run(agent, content.ERROR)
        print(result.final_output)
