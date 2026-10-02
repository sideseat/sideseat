from agent import build_agent
from llama_index.core.llms import ChatMessage, DocumentBlock, ImageBlock, TextBlock

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run.llm)
    prompt = ChatMessage(
        role="user",
        blocks=[
            TextBlock(text=content.FILES),
            ImageBlock(path=run.asset("img.jpg"), image_mimetype="image/jpeg"),
            DocumentBlock(path=run.asset("task.pdf"), title="task"),
        ],
    )
    with run.trace():
        print(await agent.run(user_msg=prompt))
