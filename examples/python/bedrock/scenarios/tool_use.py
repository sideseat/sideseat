from tools import converse

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        print(
            converse(
                run.llm,
                content.TOOL_USE,
                [content.get_weather, content.get_precipitation],
            )
        )
