from agents import agent
from agentscope.message import UserMsg

from harness import Run, content


async def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        assistant = agent(run)
        with run.trace(f"session-turn-{index}"):
            reply = await assistant.reply(UserMsg("user", question))
            print(reply.get_text_content())
