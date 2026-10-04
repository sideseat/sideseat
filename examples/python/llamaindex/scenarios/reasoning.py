from agent import build_agent
from models import build

from harness import Run, content


async def run(run: Run) -> None:
    # Not streamed: the OpenInference instrumentor records no output for a streamed response that
    # opens with reasoning.
    agent = build_agent(build(run.model, reasoning=True), system=None, streaming=False)
    with run.trace():
        print(await agent.run(user_msg=content.REASONING))
