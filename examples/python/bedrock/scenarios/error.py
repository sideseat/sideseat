from tools import converse

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        print(converse(run.llm, content.ERROR, [content.book_flight]))
