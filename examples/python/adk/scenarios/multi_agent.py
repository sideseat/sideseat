from conversation import Conversation
from google.adk.agents import LlmAgent
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    # ADK's agent transfer: the researcher hands the conversation to its writer sub-agent.
    writer = LlmAgent(
        name="writer",
        model=run.llm,
        description="Writes the final packing list from the researcher's findings.",
        instruction="You write the final packing list from the researcher's findings.",
    )
    researcher = LlmAgent(
        name="researcher",
        model=run.llm,
        instruction="You research weather with the tool, then hand off to the writer.",
        tools=[get_weather],
        sub_agents=[writer],
    )
    with run.trace():
        print(await Conversation(run, researcher).ask(content.MULTI_AGENT))
