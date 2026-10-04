from tools import converse, definitions

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        answer = converse(
            run.llm,
            [{"role": "user", "content": content.TOOL_USE}],
            tools=definitions("get_weather", "get_precipitation"),
        )
        print(answer)
