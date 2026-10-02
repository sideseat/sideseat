from crew import kickoff, one_task_crew, travel_agent

from harness import Run, content


def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        crew = one_task_crew(travel_agent(run.llm), question)
        with run.trace(f"session-turn-{index}"):
            print(kickoff(crew).raw)
