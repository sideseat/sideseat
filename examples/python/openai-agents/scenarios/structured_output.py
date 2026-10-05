from agents import Agent, Runner

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(
        name="planner",
        instructions=content.SYSTEM,
        model=run.llm,
        output_type=content.TripPlan,
    )
    with run.trace():
        result = await Runner.run(agent, content.STRUCTURED)
        print(result.final_output_as(content.TripPlan))
