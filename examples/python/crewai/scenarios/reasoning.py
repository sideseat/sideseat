from crew import kickoff, one_task_crew, travel_agent
from models import build

from harness import Run, content


def run(run: Run) -> None:
    crew = one_task_crew(
        travel_agent(build(run.model, reasoning=True)), content.REASONING
    )
    with run.trace():
        print(kickoff(crew).raw)
