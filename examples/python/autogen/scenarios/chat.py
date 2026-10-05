from agent import answer, assistant

from harness import Run, content


async def run(run: Run) -> None:
    with run.trace():
        agent = assistant(run.llm)
        print(answer(await agent.run(task=content.CHAT)))
