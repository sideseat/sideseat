from crew import kickoff, one_task_crew, travel_agent
from tools import book_flight

from harness import Run, content


def run(run: Run) -> None:
    # CrewAI reports the tool's exception to the model as the tool's result.
    agent = travel_agent(run.llm, tools=[book_flight])
    crew = one_task_crew(agent, content.ERROR)
    with run.trace():
        print(kickoff(crew).raw)
