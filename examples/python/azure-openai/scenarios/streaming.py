from tools import converse, definitions, system

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        converse(
            run.llm,
            [system(), {"role": "user", "content": content.STREAMING}],
            tools=definitions("get_weather"),
            stream=True,
        )
