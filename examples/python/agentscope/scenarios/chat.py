from agents import agent
from agentscope.message import UserMsg

from harness import Run, content


async def run(run: Run) -> None:
    assistant = agent(run)
    with run.trace():
        reply = await assistant.reply(UserMsg("user", content.CHAT))
        print(reply.get_text_content())
