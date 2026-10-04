from google.genai import types

from harness import Run, content


def run(run: Run) -> None:
    gemini = run.llm
    with run.trace():
        response = gemini.client.models.generate_content(
            model=gemini.model,
            contents=content.CHAT,
            config=types.GenerateContentConfig(system_instruction=content.SYSTEM),
        )
        print(response.text)
