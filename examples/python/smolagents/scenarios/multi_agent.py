from smolagents import ToolCallingAgent
from tools import get_weather

from harness import Run, content


def run(run: Run) -> None:
    # A manager agent that hands the research to a managed researcher agent.
    researcher = ToolCallingAgent(
        tools=[get_weather],
        model=run.llm,
        name="researcher",
        description="Researches the weather forecast for a city with the weather tool.",
        instructions="You research weather with the tool and report the forecast.",
    )
    writer = ToolCallingAgent(
        tools=[],
        model=run.llm,
        managed_agents=[researcher],
        instructions="You write the final packing list. Ask the researcher for the weather first.",
    )
    with run.trace():
        print(writer.run(content.MULTI_AGENT))
