from agents import agent, answer

from harness import Run, content


async def run(run: Run) -> None:
    # Browser Use continues a finished agent with add_new_task: the follow-up keeps the history.
    first, *follow_ups = content.MULTI_TURN
    with run.trace():
        browser = agent(run, first)
        print(answer(await browser.run(max_steps=10)))
        for question in follow_ups:
            browser.add_new_task(question)
            print(answer(await browser.run(max_steps=10)))
