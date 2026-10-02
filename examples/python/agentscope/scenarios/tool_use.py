from agents import agent
from agentscope.message import UserMsg
from agentscope.tool import Toolkit
from tools import get_precipitation, get_weather

from harness import Run, content


async def run(run: Run) -> None:
    assistant = agent(run, toolkit=Toolkit(tools=[get_weather, get_precipitation]))
    with run.trace():
        reply = await assistant.reply(UserMsg("user", content.TOOL_USE))
        print(reply.get_text_content())
