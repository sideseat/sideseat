from agent import build_agent
from langchain_core.messages import AIMessageChunk, HumanMessage
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run.llm, tools=[get_weather])
    with run.trace():
        async for chunk, _ in agent.astream(
            {"messages": [HumanMessage(content.STREAMING)]}, stream_mode="messages"
        ):
            if isinstance(chunk, AIMessageChunk):
                print(chunk.text, end="", flush=True)
        print()
