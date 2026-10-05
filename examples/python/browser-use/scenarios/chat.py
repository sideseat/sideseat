from agents import agent, answer

from harness import Run, content


async def run(run: Run) -> None:
    # A question the model answers from what it knows: one step that ends with Browser Use's done action.
    with run.trace():
        print(answer(await agent(run, content.CHAT).run(max_steps=3)))
