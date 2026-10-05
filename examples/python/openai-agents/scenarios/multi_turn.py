from typing import Any

from agents import Agent, Runner

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(name="assistant", instructions=content.SYSTEM, model=run.llm)
    history: list[Any] = []
    with run.trace():
        for question in content.MULTI_TURN:
            # The SDK's documented manual history: the previous run's items, then the new question.
            result = await Runner.run(
                agent, [*history, {"role": "user", "content": question}]
            )
            history = result.to_input_list()
            print(result.final_output)
