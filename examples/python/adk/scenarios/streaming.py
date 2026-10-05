from conversation import Conversation
from google.adk.agents import LlmAgent
from google.adk.agents.run_config import RunConfig, StreamingMode
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    agent = LlmAgent(
        name="assistant",
        model=run.llm,
        instruction=content.SYSTEM,
        tools=[get_weather],
    )
    conversation = Conversation(
        run, agent, run_config=RunConfig(streaming_mode=StreamingMode.SSE)
    )
    with run.trace():
        print(await conversation.ask(content.STREAMING))
