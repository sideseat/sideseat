from google.genai import types
from tools import get_precipitation, get_weather

from harness import Run, content


def run(run: Run) -> None:
    gemini = run.llm
    with run.trace():
        # Python functions as tools: the client calls them and returns their results to the model.
        response = gemini.client.models.generate_content(
            model=gemini.model,
            contents=content.TOOL_USE,
            config=types.GenerateContentConfig(
                system_instruction=content.SYSTEM,
                tools=[get_weather, get_precipitation],
            ),
        )
        print(response.text)
