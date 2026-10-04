from agents import agent
from converse import Conversation

from harness import Run, content


async def run(run: Run) -> None:
    assistant = agent("travel-assistant", Conversation(run.llm, content.SYSTEM))
    with run.trace():
        for question in content.MULTI_TURN:
            print(await assistant(question))
