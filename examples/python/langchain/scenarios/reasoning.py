from agent import converse
from langchain_core.messages import BaseMessage, HumanMessage
from models import build

from harness import Run, content


async def run(run: Run) -> None:
    llm = build(run.model, reasoning=True)
    messages: list[BaseMessage] = [HumanMessage(content.REASONING)]
    with run.trace():
        print((await converse(llm, messages)).text)
