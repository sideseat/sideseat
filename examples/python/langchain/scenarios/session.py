from agent import converse
from langchain_core.messages import BaseMessage, HumanMessage, SystemMessage

from harness import Run, content


async def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            messages: list[BaseMessage] = [
                SystemMessage(content.SYSTEM),
                HumanMessage(question),
            ]
            print((await converse(run.llm, messages)).text)
