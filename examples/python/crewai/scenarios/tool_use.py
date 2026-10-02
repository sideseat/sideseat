from crew import kickoff, one_task_crew, travel_agent
from tools import get_precipitation, get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = travel_agent(run.llm, tools=[get_weather, get_precipitation])
    crew = one_task_crew(agent, content.TOOL_USE)
    with run.trace():
        print(kickoff(crew).raw)
