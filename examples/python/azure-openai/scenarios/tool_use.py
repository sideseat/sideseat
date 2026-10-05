from tools import converse, definitions, system

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        answer = converse(
            run.llm,
            [system(), {"role": "user", "content": content.TOOL_USE}],
            tools=definitions("get_weather", "get_precipitation"),
        )
        print(answer)
