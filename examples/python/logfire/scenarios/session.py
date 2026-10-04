from agents import agent
from conversation import Conversation

from harness import Run, content


async def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        assistant = agent("travel-assistant", Conversation(run.llm, content.SYSTEM))
        with run.trace(f"session-turn-{index}"):
            print(await assistant(question))
