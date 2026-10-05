from tools import converse, system

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        print(converse(run.llm, [system(), {"role": "user", "content": content.CHAT}]))
