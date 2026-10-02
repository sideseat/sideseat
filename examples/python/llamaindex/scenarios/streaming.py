from agent import build_agent
from llama_index.core.agent.workflow import AgentStream
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run.llm, tools=[get_weather], streaming=True)
    with run.trace():
        handler = agent.run(user_msg=content.STREAMING)
        async for event in handler.stream_events():
            if isinstance(event, AgentStream):
                print(event.delta, end="", flush=True)
        await handler
        print()
