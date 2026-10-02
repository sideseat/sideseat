from crew import kickoff
from crewai import Agent, Crew, Process, Task
from tools import get_weather

from harness import Run, content


def run(run: Run) -> None:
    researcher = Agent(
        role="Weather researcher",
        goal="Find the forecast the traveller needs",
        backstory="You research weather with the tool, then hand off to the writer.",
        llm=run.llm,
        tools=[get_weather],
        verbose=False,
    )
    writer = Agent(
        role="Packing list writer",
        goal="Write the packing list",
        backstory="You write the final packing list from the researcher's findings.",
        llm=run.llm,
        verbose=False,
    )
    research = Task(
        description=content.MULTI_AGENT,
        expected_output="The forecast for each day.",
        agent=researcher,
    )
    # The sequential process hands the research task's output to the writing task as context.
    write = Task(
        description="Write the one-paragraph packing list the request asks for.",
        expected_output="A one-paragraph packing list.",
        agent=writer,
        context=[research],
    )
    crew = Crew(
        agents=[researcher, writer],
        tasks=[research, write],
        process=Process.sequential,
        verbose=False,
    )
    with run.trace():
        print(kickoff(crew).raw)
