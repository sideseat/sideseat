from conversation import Conversation
from google.adk.agents import LlmAgent
from models import build

from harness import Run, content


async def run(run: Run) -> None:
    agent = LlmAgent(name="assistant", model=build(run.model, reasoning=True))
    with run.trace():
        print(await Conversation(run, agent).ask(content.REASONING))
