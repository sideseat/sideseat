from agent import answer, ask, build_agent
from haystack.dataclasses import ChatMessage, FileContent, ImageContent

from harness import Run, content


def run(run: Run) -> None:
    agent = build_agent(run.llm)
    prompt = ChatMessage.from_user(
        content_parts=[
            content.FILES,
            ImageContent.from_file_path(run.asset("img.jpg")),
            FileContent.from_file_path(run.asset("task.pdf")),
        ]
    )
    with run.trace():
        print(answer(ask(agent, prompt)))
