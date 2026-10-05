from conversation import Conversation
from google.adk.agents import LlmAgent

from harness import Run, content


async def run(run: Run) -> None:
    agent = LlmAgent(name="assistant", model=run.llm, instruction=content.SYSTEM)
    conversation = Conversation(run, agent)
    with run.trace():
        for question in content.MULTI_TURN:
            print(await conversation.ask(question))
