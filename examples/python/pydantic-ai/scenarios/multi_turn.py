from pydantic_ai import Agent
from pydantic_ai.messages import ModelMessage

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(run.llm, instructions=content.SYSTEM)
    history: list[ModelMessage] = []
    with run.trace():
        for question in content.MULTI_TURN:
            result = agent.run_sync(question, message_history=history)
            history = result.all_messages()
            print(result.output)
