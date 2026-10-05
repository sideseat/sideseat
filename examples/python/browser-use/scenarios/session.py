from agents import agent, answer

from harness import Run, content


async def run(run: Run) -> None:
    # Two tasks, each its own agent and trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            print(answer(await agent(run, question).run(max_steps=3)))
