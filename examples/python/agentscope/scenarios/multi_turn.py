from agents import agent
from agentscope.message import UserMsg

from harness import Run, content


async def run(run: Run) -> None:
    # The agent keeps its context between replies, so each request re-sends the earlier turns.
    assistant = agent(run)
    with run.trace():
        for question in content.MULTI_TURN:
            reply = await assistant.reply(UserMsg("user", question))
            print(reply.get_text_content())
