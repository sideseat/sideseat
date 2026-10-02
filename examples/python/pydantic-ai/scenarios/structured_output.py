from pydantic_ai import Agent

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(run.llm, instructions=content.SYSTEM, output_type=content.TripPlan)
    with run.trace():
        print(agent.run_sync(content.STRUCTURED).output)
