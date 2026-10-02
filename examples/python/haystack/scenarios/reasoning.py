from agent import answer, ask, build_agent
from haystack.dataclasses import ChatMessage
from models import build

from harness import Run, content


def run(run: Run) -> None:
    agent = build_agent(build(run.model, reasoning=True), system=None)
    with run.trace():
        print(answer(ask(agent, ChatMessage.from_user(content.REASONING))))
