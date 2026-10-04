from agents import agent
from converse import Conversation
from tools import book_flight

from harness import Run, content


async def run(run: Run) -> None:
    conversation = Conversation(run.llm, content.SYSTEM, [book_flight])
    with run.trace():
        print(await agent("travel-assistant", conversation)(content.ERROR))
