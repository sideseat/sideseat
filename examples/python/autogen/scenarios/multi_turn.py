from agent import answer, assistant

from harness import Run, content


async def run(run: Run) -> None:
    with run.trace():
        # The agent keeps its model context between runs, so each question sends the history before it.
        agent = assistant(run.llm)
        for question in content.MULTI_TURN:
            print(answer(await agent.run(task=question)))
