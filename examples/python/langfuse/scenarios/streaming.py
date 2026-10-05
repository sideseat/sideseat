from tools import definitions, respond

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        respond(
            run.llm,
            [{"role": "user", "content": content.STREAMING}],
            tools=definitions("get_weather"),
            stream=True,
        )
