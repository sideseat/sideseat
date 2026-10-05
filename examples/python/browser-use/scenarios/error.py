from agents import agent, answer
from tools import tools

from harness import Run, content


async def run(run: Run) -> None:
    # book_flight raises; Browser Use reports the error to the model in the next step's history.
    with run.trace():
        browser = agent(run, content.ERROR, tools=tools(content.book_flight))
        print(answer(await browser.run(max_steps=6)))
