from tools import respond

from harness import Run, content


def run(run: Run) -> None:
    with run.trace():
        # No function of the application's: the provider runs its own web search and answers from it.
        answer = respond(
            run.llm,
            [{"role": "user", "content": content.SERVER_TOOLS}],
            tools=[{"type": "web_search"}],
        )
        print(answer)
