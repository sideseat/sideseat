from agent import ask, build_agent
from haystack.dataclasses import ChatMessage
from models import build
from tools import get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = build_agent(build(run.model, streaming=True), tools=[get_weather])
    with run.trace():
        ask(agent, ChatMessage.from_user(content.STREAMING))
        print()
