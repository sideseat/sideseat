from agents import agent
from converse import Conversation
from tools import get_weather
from traceloop.sdk.decorators import workflow

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

    @workflow(name="packing-list")
    async def plan(task: str) -> str:
        # The researcher's findings become the writer's brief: a handoff between two agents.
        findings = await researcher(task)
        return await writer(f"{task}\n\nResearcher's findings:\n{findings}")

    with run.trace():
        print(await plan(content.MULTI_AGENT))
