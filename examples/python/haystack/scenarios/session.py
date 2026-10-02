from agent import answer, ask, build_agent
from haystack.dataclasses import ChatMessage

from harness import Run, content


def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    agent = build_agent(run.llm)
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            print(answer(ask(agent, ChatMessage.from_user(question))))
