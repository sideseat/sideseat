from conversation import Conversation
from google.adk.agents import LlmAgent

from harness import Run, content


async def run(run: Run) -> None:
    # Two traces in one ADK session, attributed to one session and user. An ADK session keeps its
    # history, so the second request re-sends the first trace's turn.
    agent = LlmAgent(name="assistant", model=run.llm, instruction=content.SYSTEM)
    conversation = Conversation(run, agent)
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            print(await conversation.ask(question))
