from agent import answer, assistant
from models import build

from harness import Run, content


async def run(run: Run) -> None:
    agent = assistant(build(run.model, reasoning=True), system=None)
    with run.trace():
        print(answer(await agent.run(task=content.REASONING)))
