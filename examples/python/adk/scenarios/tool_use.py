from conversation import Conversation
from google.adk.agents import LlmAgent
from tools import get_precipitation, get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = LlmAgent(
        name="assistant",
        model=run.llm,
        instruction=content.SYSTEM,
        tools=[get_weather, get_precipitation],
    )
    with run.trace():
        print(await Conversation(run, agent).ask(content.TOOL_USE))
