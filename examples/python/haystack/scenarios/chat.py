from agent import answer, ask, build_agent
from haystack.dataclasses import ChatMessage

from harness import Run, content


def run(run: Run) -> None:
    agent = build_agent(run.llm)
    with run.trace():
        print(answer(ask(agent, ChatMessage.from_user(content.CHAT))))
