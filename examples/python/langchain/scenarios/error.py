from agent import converse
from langchain_core.messages import BaseMessage, HumanMessage, SystemMessage
from tools import book_flight

from harness import Run, content


async def run(run: Run) -> None:
    messages: list[BaseMessage] = [
        SystemMessage(content.SYSTEM),
        HumanMessage(content.ERROR),
    ]
    with run.trace():
        print((await converse(run.llm, messages, tools=[book_flight])).text)
