from agents import agent
from agentscope.message import UserMsg
from models import build

from harness import Run, content


async def run(run: Run) -> None:
    assistant = agent(
        run,
        system_prompt="You solve puzzles step by step.",
        model=build(run.model, reasoning=True),
    )
    with run.trace():
        reply = await assistant.reply(UserMsg("user", content.REASONING))
        print(reply.get_text_content())
