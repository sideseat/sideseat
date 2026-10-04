from agents import agent
from converse import Conversation

from harness import Run, content

# Current Claude models think by default and omit the text; a summary makes it visible.
THINKING = {
    "additionalModelRequestFields": {
        "thinking": {"type": "adaptive", "display": "summarized"}
    },
    "outputConfig": {"effort": "max"},
}


async def run(run: Run) -> None:
    solver = agent("solver", Conversation(run.llm, fields=THINKING))
    with run.trace():
        print(await solver(content.REASONING))
