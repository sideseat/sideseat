from google.genai import types
from tools import book_flight

from harness import Run, content


def run(run: Run) -> None:
    gemini = run.llm
    with run.trace():
        # The client catches the tool's exception and sends it to the model as the call's error.
        response = gemini.client.models.generate_content(
            model=gemini.model,
            contents=content.ERROR,
            config=types.GenerateContentConfig(
                system_instruction=content.SYSTEM, tools=[book_flight]
            ),
        )
        print(response.text)
