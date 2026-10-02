"""The agent and one-task crew the scenarios run.

CrewAI builds an agent's system prompt from its role, goal, and backstory; the shared system prompt
is the backstory.
"""

from collections.abc import Sequence
from typing import Any

from crewai import LLM, Agent, Crew, CrewOutput, Task
from crewai.tools import BaseTool

from harness import content


def travel_agent(llm: LLM, *, tools: Sequence[BaseTool] = (), **options: Any) -> Agent:
    return Agent(
        role="Travel assistant",
        goal="Answer the traveller's question",
        backstory=content.SYSTEM,
        llm=llm,
        tools=list(tools),
        verbose=False,
        **options,
    )


def one_task_crew(agent: Agent, request: str, **task_options: Any) -> Crew:
    task = Task(
        description=request,
        expected_output="A direct answer to the request.",
        agent=agent,
        **task_options,
    )
    return Crew(agents=[agent], tasks=[task], verbose=False)


def kickoff(crew: Crew) -> CrewOutput:
    """Runs a crew that does not stream."""
    output = crew.kickoff()
    assert isinstance(output, CrewOutput)
    return output
