from agent import answer
from autogen_agentchat.agents import AssistantAgent
from autogen_agentchat.conditions import SourceMatchTermination
from autogen_agentchat.teams import Swarm
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    researcher = AssistantAgent(
        "researcher",
        model_client=run.llm,
        system_message="You research weather with the tool, then hand off to the writer.",
        tools=[get_weather],
        handoffs=["writer"],
    )
    writer = AssistantAgent(
        "writer",
        model_client=run.llm,
        system_message="You write the final packing list from the researcher's findings.",
    )
    team = Swarm(
        [researcher, writer], termination_condition=SourceMatchTermination(["writer"])
    )
    with run.trace():
        print(answer(await team.run(task=content.MULTI_AGENT)))
