from crew import kickoff, one_task_crew, travel_agent
from crewai_files import ImageFile, PDFFile

from harness import Run, content


def run(run: Run) -> None:
    crew = one_task_crew(
        travel_agent(run.llm),
        content.FILES,
        input_files={
            "image": ImageFile(source=str(run.asset("img.jpg"))),
            "task": PDFFile(source=str(run.asset("task.pdf"))),
        },
    )
    with run.trace():
        print(kickoff(crew).raw)
