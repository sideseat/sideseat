from agent import answer, ask, build_agent
from haystack.dataclasses import ChatMessage
from tools import book_flight

from harness import Run, content


def run(run: Run) -> None:
    # The agent returns the tool's exception to the model as an error result.
    agent = build_agent(run.llm, tools=[book_flight])
    with run.trace():
        print(answer(ask(agent, ChatMessage.from_user(content.ERROR))))
