from agno.agent import Agent
from agno.team import Team
from tools import get_weather
from results import answer

from harness import Run, content


def run(run: Run) -> None:
    # A coordinating team: the leader delegates the research, then the writing, to its members.
    researcher = Agent(
        name="researcher",
        role="Researches weather with the tool",
        model=run.llm,
        instructions="You research weather with the tool and report the forecast.",
        tools=[get_weather],
    )
    writer = Agent(
        name="writer",
        role="Writes the packing list",
        model=run.llm,
        instructions="You write the final packing list from the researcher's findings.",
    )
    team = Team(
        name="travel-team",
        model=run.llm,
        members=[researcher, writer],
        instructions="Ask the researcher for the forecast, then ask the writer for the packing list.",
    )
    with run.trace():
        result = team.run(
            content.MULTI_AGENT, session_id=run.session_id, user_id=run.user_id
        )
        print(answer(result))
