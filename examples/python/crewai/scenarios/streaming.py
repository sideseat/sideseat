from crew import travel_agent
from crewai import Crew, Task
from crewai.types.streaming import CrewStreamingOutput
from models import build
from tools import get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = travel_agent(build(run.model, stream=True), tools=[get_weather])
    task = Task(
        description=content.STREAMING,
        expected_output="A direct answer to the request.",
        agent=agent,
    )
    crew = Crew(agents=[agent], tasks=[task], stream=True, verbose=False)
    with run.trace():
        streaming = crew.kickoff()
        assert isinstance(streaming, CrewStreamingOutput)
        for chunk in streaming:
            print(chunk.content, end="", flush=True)
        print()
