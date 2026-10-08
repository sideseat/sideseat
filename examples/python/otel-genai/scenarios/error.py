from tools import definitions, respond

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        answer = respond(
            run.llm,
            [{"role": "user", "content": content.ERROR}],
            tools=definitions("book_flight"),
        )
        print(answer)
