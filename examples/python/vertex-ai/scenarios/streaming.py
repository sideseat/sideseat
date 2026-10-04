from google.genai import types
from tools import get_weather

from harness import Run, content


def run(run: Run) -> None:
    gemini = run.llm
    with run.trace():
        stream = gemini.client.models.generate_content_stream(
            model=gemini.model,
            contents=content.STREAMING,
            config=types.GenerateContentConfig(
                system_instruction=content.SYSTEM, tools=[get_weather]
            ),
        )
        for chunk in stream:
            if chunk.text:
                print(chunk.text, end="", flush=True)
        print()
