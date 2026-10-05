from typing import Any

from tools import chat, system

from harness import Run, content


def run(run: Run) -> None:
    messages: list[dict[str, Any]] = [system()]
    with run.trace():
        for question in content.MULTI_TURN:
            messages.append({"role": "user", "content": question})
            print(chat(run.llm, messages))
