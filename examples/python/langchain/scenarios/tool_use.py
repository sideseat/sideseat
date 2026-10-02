from agent import converse
from langchain_core.messages import BaseMessage, HumanMessage, SystemMessage
from tools import get_precipitation, get_weather

from harness import Run, content


async def run(run: Run) -> None:
    messages: list[BaseMessage] = [
        SystemMessage(content.SYSTEM),
        HumanMessage(content.TOOL_USE),
    ]
    with run.trace():
        reply = await converse(
            run.llm, messages, tools=[get_weather, get_precipitation]
        )
        print(reply.text)
