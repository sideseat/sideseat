from agent_framework import Agent
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    # An agent used as another agent's tool, as Agent Framework documents: the writer hands the research
    # to the researcher and writes the list from its answer.
    researcher = Agent(
        client=run.llm,
        name="researcher",
        description="Researches the weather forecast for a city.",
        instructions="You research weather with the tool and report the forecast.",
        tools=[get_weather],
    )
    writer = Agent(
        client=run.llm,
        name="writer",
        instructions="You write the final packing list. Ask the researcher for the weather first.",
        tools=[researcher.as_tool()],
    )
    session = writer.create_session(session_id=run.session_id)
    with run.trace():
        print((await writer.run(content.MULTI_AGENT, session=session)).text)
