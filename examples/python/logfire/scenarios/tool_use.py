from agents import agent
from conversation import Conversation
from tools import get_precipitation, get_weather

from harness import Run, content


async def run(run: Run) -> None:
    conversation = Conversation(
        run.llm, content.SYSTEM, [get_weather, get_precipitation]
    )
    with run.trace():
        print(await agent("travel-assistant", conversation)(content.TOOL_USE))
