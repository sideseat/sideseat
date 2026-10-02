from agent import build_agent
from models import build

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(build(run.model, reasoning=True), system=None)
    with run.trace():
        print(await agent.run(user_msg=content.REASONING))
