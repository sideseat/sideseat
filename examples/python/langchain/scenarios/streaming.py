from agent import converse
from langchain_core.messages import BaseMessage, HumanMessage, SystemMessage
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    messages: list[BaseMessage] = [
        SystemMessage(content.SYSTEM),
        HumanMessage(content.STREAMING),
    ]
    with run.trace():
        await converse(run.llm, messages, tools=[get_weather], stream=True)
