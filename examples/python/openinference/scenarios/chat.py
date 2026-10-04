from agents import agent
from converse import Conversation

from harness import Run, content


async def run(run: Run) -> None:
    assistant = agent("travel-assistant", Conversation(run.llm, content.SYSTEM))
    with run.trace():
        print(await assistant(content.CHAT))
