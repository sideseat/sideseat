from tools import definitions, respond

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        answer = respond(
            run.llm,
            [{"role": "user", "content": content.TOOL_USE}],
            tools=definitions("get_weather", "get_precipitation"),
        )
        print(answer)
