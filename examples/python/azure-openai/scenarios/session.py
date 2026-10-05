from tools import converse, system

from harness import Run, content


def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            print(converse(run.llm, [system(), {"role": "user", "content": question}]))
