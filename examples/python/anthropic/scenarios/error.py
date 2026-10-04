from tools import converse, definitions

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        answer = converse(
            run.llm,
            [{"role": "user", "content": content.ERROR}],
            tools=definitions("book_flight"),
        )
        print(answer)
