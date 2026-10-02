from strands import Agent
from strands.multiagent import Swarm
from tools import get_weather

from harness import Run, content


def run(run: Run) -> None:
    researcher = Agent(
        name="researcher",
        model=run.llm,
        system_prompt="You research weather with the tool, then hand off to the writer.",
        tools=[get_weather],
        callback_handler=None,
    )
    writer = Agent(
        name="writer",
        model=run.llm,
        system_prompt="You write the final packing list from the researcher's findings.",
        callback_handler=None,
    )
    swarm = Swarm([researcher, writer], entry_point=researcher, max_handoffs=4)
    with run.trace():
        print(swarm(content.MULTI_AGENT))
