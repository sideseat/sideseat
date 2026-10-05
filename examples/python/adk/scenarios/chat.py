from conversation import Conversation
from google.adk.agents import LlmAgent

from harness import Run, content


async def run(run: Run) -> None:
    agent = LlmAgent(name="assistant", model=run.llm, instruction=content.SYSTEM)
    with run.trace():
        print(await Conversation(run, agent).ask(content.CHAT))
