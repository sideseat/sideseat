from agent import answer, assistant

from harness import Run, content


async def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        agent = assistant(run.llm)
        with run.trace(f"session-turn-{index}"):
            print(answer(await agent.run(task=question)))
