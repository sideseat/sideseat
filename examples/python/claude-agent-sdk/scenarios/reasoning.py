from agent import options, show
from claude_agent_sdk import query

from harness import Run, content


async def run(run: Run) -> None:
    # Current Claude models think by default and omit the text; a summary makes it visible.
    settings = options(
        run,
        system_prompt=None,
        max_turns=1,
        thinking={"type": "adaptive", "display": "summarized"},
        effort="max",
    )
    with run.trace():
        await show(query(prompt=content.REASONING, options=settings))
