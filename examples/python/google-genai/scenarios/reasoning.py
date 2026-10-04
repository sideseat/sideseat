from google.genai import types

from harness import Run, content


def run(run: Run) -> None:
    gemini = run.llm
    with run.trace():
        response = gemini.client.models.generate_content(
            model=gemini.model,
            contents=content.REASONING,
            # Thought summaries are returned only when asked for.
            config=types.GenerateContentConfig(
                thinking_config=types.ThinkingConfig(include_thoughts=True)
            ),
        )
        print(response.text)
