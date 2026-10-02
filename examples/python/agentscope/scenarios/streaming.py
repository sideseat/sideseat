from agents import agent
from agentscope.event import TextBlockDeltaEvent
from agentscope.message import UserMsg
from agentscope.tool import Toolkit
from models import build
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    assistant = agent(
        run, model=build(run.model, stream=True), toolkit=Toolkit(tools=[get_weather])
    )
    with run.trace():
        async for event in assistant.reply_stream(UserMsg("user", content.STREAMING)):
            if isinstance(event, TextBlockDeltaEvent):
                print(event.delta, end="", flush=True)
        print()
