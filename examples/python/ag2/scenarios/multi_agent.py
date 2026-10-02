from ag2.tools.subagents import subagent_tool
from agent import build_agent
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    writer = build_agent(
        run,
        "writer",
        prompt="You write the final packing list from the researcher's findings.",
    )
    # The researcher hands off by delegating to the writer as a sub-task.
    researcher = build_agent(
        run,
        "researcher",
        prompt="You research weather with the tool, then hand off to the writer.",
        tools=[
            get_weather,
            subagent_tool(
                writer, description="Write the packing list from the findings."
            ),
        ],
    )
    with run.trace():
        print(await (await researcher.ask(content.MULTI_AGENT)).content())
