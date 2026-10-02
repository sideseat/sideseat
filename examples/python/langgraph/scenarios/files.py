import base64

from agent import answer, build_agent
from langchain_core.messages import HumanMessage

from harness import Run, content


def _base64(name: str) -> str:
    return base64.b64encode(Run.asset(name).read_bytes()).decode("ascii")


async def run(run: Run) -> None:
    agent = build_agent(run.llm)
    prompt = HumanMessage(
        content=[
            {"type": "text", "text": content.FILES},
            {"type": "image", "base64": _base64("img.jpg"), "mime_type": "image/jpeg"},
            {
                "type": "file",
                "base64": _base64("task.pdf"),
                "mime_type": "application/pdf",
                "name": "task",
            },
        ]
    )
    with run.trace():
        print(answer(await agent.ainvoke({"messages": [prompt]})))
