from typing import Any

from tools import respond

from harness import Run, content


def run(run: Run) -> None:
    items: list[dict[str, Any]] = []
    with run.trace():
        for question in content.MULTI_TURN:
            items.append({"role": "user", "content": question})
            print(respond(run.llm, items))
