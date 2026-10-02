from typing import Any

from crew import travel_agent

from harness import Run, content


async def run(run: Run) -> None:
    # A crew runs tasks, not conversations; an agent kicked off with the message history is how
    # CrewAI holds one.
    agent = travel_agent(run.llm)
    history: list[Any] = []
    with run.trace():
        for question in content.MULTI_TURN:
            history.append({"role": "user", "content": question})
            reply = (await agent.kickoff_async(history)).raw
            history.append({"role": "assistant", "content": reply})
            print(reply)
