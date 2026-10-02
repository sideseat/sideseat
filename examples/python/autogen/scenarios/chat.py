from agent import answer, assistant

from harness import Run, content


async def run(run: Run) -> None:
    agent = assistant(run.llm)
    with run.trace():
        print(answer(await agent.run(task=content.CHAT)))
