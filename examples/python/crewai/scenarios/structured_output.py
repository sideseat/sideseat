from crew import kickoff, one_task_crew, travel_agent

from harness import Run, content


def run(run: Run) -> None:
    crew = one_task_crew(
        travel_agent(run.llm), content.STRUCTURED, output_pydantic=content.TripPlan
    )
    with run.trace():
        print(kickoff(crew).pydantic)
