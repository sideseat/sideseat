import logfire
from agents import agent
from conversation import Conversation
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    researcher = agent(
        "researcher",
        Conversation(
            run.llm,
            "You research weather with the tool and report the forecast.",
            [get_weather],
        ),
    )
    writer = agent(
        "writer",
        Conversation(
            run.llm, "You write the final packing list from the researcher's findings."
        ),
    )
    with run.trace(), logfire.span("packing list"):
        # The researcher's findings become the writer's brief: a handoff between two agents.
        findings = await researcher(content.MULTI_AGENT)
        print(
            await writer(f"{content.MULTI_AGENT}\n\nResearcher's findings:\n{findings}")
        )
