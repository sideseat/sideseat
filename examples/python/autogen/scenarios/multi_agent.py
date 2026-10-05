from agent import answer
from autogen_agentchat.agents import AssistantAgent
from autogen_agentchat.conditions import SourceMatchTermination
from autogen_agentchat.teams import RoundRobinGroupChat
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    with run.trace():
        researcher = AssistantAgent(
            "researcher",
            model_client=run.llm,
            system_message="You research weather with the tool, then hand off to the writer.",
            tools=[get_weather],
            reflect_on_tool_use=True,
        )
        writer = AssistantAgent(
            "writer",
            model_client=run.llm,
            system_message="You write the final packing list from the researcher's findings.",
        )
        # The team passes the turn in order: the researcher's findings, then the writer's list.
        team = RoundRobinGroupChat(
            [researcher, writer],
            termination_condition=SourceMatchTermination(["writer"]),
        )
        print(answer(await team.run(task=content.MULTI_AGENT)))
