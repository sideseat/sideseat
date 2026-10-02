from agent import answer, assistant
from tools import book_flight

from harness import Run, content


async def run(run: Run) -> None:
    # The tool's exception becomes an error result, which the reflection step answers.
    agent = assistant(run.llm, tools=[book_flight])
    with run.trace():
        print(answer(await agent.run(task=content.ERROR)))
