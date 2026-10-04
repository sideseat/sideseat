from crew import kickoff, travel_agent
from crewai import Crew, Task

from harness import Run, content


def run(run: Run) -> None:
    # CrewAI holds a conversation as tasks: each question is a task that takes the earlier ones,
    # already answered, as context, so its request carries the conversation so far. One crew per
    # question, so each kickoff is one turn.
    agent = travel_agent(run.llm)
    answered: list[Task] = []
    with run.trace():
        for question in content.MULTI_TURN:
            task = Task(
                description=question,
                expected_output="A direct answer to the request.",
                agent=agent,
                context=list(answered),
            )
            print(kickoff(Crew(agents=[agent], tasks=[task], verbose=False)).raw)
            answered.append(task)
