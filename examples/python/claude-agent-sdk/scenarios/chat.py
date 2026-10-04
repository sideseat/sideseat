from agent import options, show
from claude_agent_sdk import query

from harness import Run, content


async def run(run: Run) -> None:
    with run.trace():
        await show(query(prompt=content.CHAT, options=options(run, max_turns=1)))
