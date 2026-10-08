from tools import respond

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        print(respond(run.llm, [{"role": "user", "content": content.CHAT}]))
