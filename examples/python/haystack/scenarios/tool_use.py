from agent import answer, ask, build_agent
from haystack.dataclasses import ChatMessage
from tools import get_precipitation, get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = build_agent(run.llm, tools=[get_weather, get_precipitation])
    with run.trace():
        print(answer(ask(agent, ChatMessage.from_user(content.TOOL_USE))))
