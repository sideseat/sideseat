from agent import converse
from langchain_core.messages import BaseMessage, HumanMessage, SystemMessage

from harness import Run, content


async def run(run: Run) -> None:
    # The application keeps the history and sends all of it with each question.
    history: list[BaseMessage] = [SystemMessage(content.SYSTEM)]
    with run.trace():
        for question in content.MULTI_TURN:
            history.append(HumanMessage(question))
            print((await converse(run.llm, history)).text)
