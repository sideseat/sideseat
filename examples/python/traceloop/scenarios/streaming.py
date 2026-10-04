from agents import agent
from converse import Conversation
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    conversation = Conversation(run.llm, content.SYSTEM, [get_weather], stream=True)
    with run.trace():
        await agent("travel-assistant", conversation)(content.STREAMING)
