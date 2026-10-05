from agent import assistant
from autogen_agentchat.messages import ModelClientStreamingChunkEvent
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    with run.trace():
        agent = assistant(run.llm, tools=[get_weather], model_client_stream=True)
        async for event in agent.run_stream(task=content.STREAMING):
            if isinstance(event, ModelClientStreamingChunkEvent):
                print(event.content, end="", flush=True)
        print()
