from google.genai import types

from harness import Run, content


def run(run: Run) -> None:
    gemini = run.llm
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            response = gemini.client.models.generate_content(
                model=gemini.model,
                contents=question,
                config=types.GenerateContentConfig(system_instruction=content.SYSTEM),
            )
            print(response.text)
