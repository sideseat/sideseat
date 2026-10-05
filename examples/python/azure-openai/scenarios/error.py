from tools import converse, definitions, system

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        answer = converse(
            run.llm,
            [system(), {"role": "user", "content": content.ERROR}],
            tools=definitions("book_flight"),
        )
        print(answer)
