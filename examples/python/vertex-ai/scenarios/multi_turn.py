from google.genai import types

from harness import Run, content


def run(run: Run) -> None:
    gemini = run.llm
    # A chat session keeps the history and re-sends it with every message.
    chat = gemini.client.chats.create(
        model=gemini.model,
        config=types.GenerateContentConfig(system_instruction=content.SYSTEM),
    )
    with run.trace():
        for question in content.MULTI_TURN:
            print(chat.send_message(question).text)
