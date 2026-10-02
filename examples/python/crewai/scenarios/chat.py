from crew import kickoff, one_task_crew, travel_agent

from harness import Run, content


def run(run: Run) -> None:
    crew = one_task_crew(travel_agent(run.llm), content.CHAT)
    with run.trace():
        print(kickoff(crew).raw)
