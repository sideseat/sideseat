from tools import chat, system

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        print(chat(run.llm, [system(), {"role": "user", "content": content.CHAT}]))
