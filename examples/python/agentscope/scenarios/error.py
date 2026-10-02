from agents import agent
from agentscope.message import UserMsg
from agentscope.tool import Toolkit
from tools import book_flight

from harness import Run, content


async def run(run: Run) -> None:
    assistant = agent(run, toolkit=Toolkit(tools=[book_flight]))
    with run.trace():
        reply = await assistant.reply(UserMsg("user", content.ERROR))
        print(reply.get_text_content())
