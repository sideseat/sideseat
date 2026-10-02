from agents import agent
from agentscope.event import TextBlockDeltaEvent
from agentscope.message import UserMsg
from agentscope.pipeline import TeamMember, TeamPipeline
from agentscope.tool import Toolkit
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    # A team pipeline: the leader assigns the research to the researcher, then writes the list itself.
    leader = agent(
        run,
        name="writer",
        system_prompt="You write the final packing list. Ask the researcher for the weather first.",
        toolkit=Toolkit(),
    )
    researcher = agent(
        run,
        name="researcher",
        system_prompt="You research weather with the tool and report the forecast.",
        toolkit=Toolkit(tools=[get_weather]),
    )
    team = TeamPipeline(
        leader=leader,
        members=[
            TeamMember(
                agent=researcher,
                description="Researches the weather forecast for a city.",
            )
        ],
    )
    with run.trace():
        async for event in team.reply_stream(UserMsg("user", content.MULTI_AGENT)):
            if isinstance(event, TextBlockDeltaEvent):
                print(event.delta, end="", flush=True)
        print()
