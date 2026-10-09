from tools import converse

from harness import Run, content

#: Anthropic's direct search: the dated release whose calls are not nested in code execution.
WEB_SEARCH = {"type": "web_search_20250305", "name": "web_search", "max_uses": 1}


def run(run: Run) -> None:
    with run.trace():
        print(
            converse(
                run.llm,
                [{"role": "user", "content": content.SERVER_TOOLS}],
                tools=[WEB_SEARCH],
            )
        )
