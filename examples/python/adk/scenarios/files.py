from conversation import Conversation
from google.adk.agents import LlmAgent
from google.genai import types

from harness import Run, content


async def run(run: Run) -> None:
    agent = LlmAgent(name="assistant", model=run.llm, instruction=content.SYSTEM)
    prompt = [
        types.Part(text=content.FILES),
        types.Part.from_bytes(
            data=run.asset("img.jpg").read_bytes(), mime_type="image/jpeg"
        ),
        types.Part.from_bytes(
            data=run.asset("task.pdf").read_bytes(), mime_type="application/pdf"
        ),
    ]
    with run.trace():
        print(await Conversation(run, agent).ask(prompt))
