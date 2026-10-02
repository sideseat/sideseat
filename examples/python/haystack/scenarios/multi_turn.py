from agent import answer, ask, build_agent
from haystack.dataclasses import ChatMessage

from harness import Run, content


def run(run: Run) -> None:
    # The agent is stateless: each run receives the conversation so far, without the system
    # prompt, which the agent adds itself.
    agent = build_agent(run.llm)
    history: list[ChatMessage] = []
    with run.trace():
        for question in content.MULTI_TURN:
            result = ask(agent, *history, ChatMessage.from_user(question))
            history = [m for m in result["messages"] if not m.is_from("system")]
            print(answer(result))
