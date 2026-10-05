from agents import Agent, Runner
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    # A handoff: the researcher passes the conversation to the writer.
    writer = Agent(
        name="writer",
        handoff_description="Writes the final packing list from the researcher's findings.",
        instructions="You write the final packing list from the researcher's findings.",
        model=run.llm,
    )
    researcher = Agent(
        name="researcher",
        instructions="You research weather with the tool, then hand off to the writer.",
        model=run.llm,
        tools=[get_weather],
        handoffs=[writer],
    )
    with run.trace():
        result = await Runner.run(researcher, content.MULTI_AGENT)
        print(result.final_output)
