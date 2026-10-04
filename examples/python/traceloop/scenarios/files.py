from agents import agent
from converse import Conversation

from harness import Run, content


async def run(run: Run) -> None:
    attachments = [
        {
            "image": {
                "format": "jpeg",
                "source": {"bytes": run.asset("img.jpg").read_bytes()},
            }
        },
        {
            "document": {
                "format": "pdf",
                "name": "task",
                "source": {"bytes": run.asset("task.pdf").read_bytes()},
            }
        },
    ]
    assistant = agent(
        "travel-assistant", Conversation(run.llm, content.SYSTEM), attachments
    )
    with run.trace():
        print(await assistant(content.FILES))
