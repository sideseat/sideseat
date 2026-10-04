from tools import converse

from harness import Run, content


def run(run: Run) -> None:
    messages: list[dict[str, object]] = []
    with run.trace():
        for question in content.MULTI_TURN:
            messages.append({"role": "user", "content": question})
            print(converse(run.llm, messages))
