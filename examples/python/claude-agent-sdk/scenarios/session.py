from agent import options, show
from claude_agent_sdk import query

from harness import Run, content


async def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            await show(query(prompt=question, options=options(run, max_turns=1)))
