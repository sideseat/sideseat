from google.genai import types

from harness import Run, content


def run(run: Run) -> None:
    gemini = run.llm
    prompt = [
        content.FILES,
        types.Part.from_bytes(
            data=run.asset("img.jpg").read_bytes(), mime_type="image/jpeg"
        ),
        types.Part.from_bytes(
            data=run.asset("task.pdf").read_bytes(), mime_type="application/pdf"
        ),
    ]
    with run.trace():
        response = gemini.client.models.generate_content(
            model=gemini.model,
            contents=prompt,
            config=types.GenerateContentConfig(system_instruction=content.SYSTEM),
        )
        print(response.text)
