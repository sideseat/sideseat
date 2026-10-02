import base64

from agents import agent
from agentscope.message import Base64Source, DataBlock, TextBlock, UserMsg

from harness import Run, content


def _data(run: Run, name: str, media_type: str) -> DataBlock:
    encoded = base64.b64encode(run.asset(name).read_bytes()).decode("ascii")
    return DataBlock(
        source=Base64Source(data=encoded, media_type=media_type), name=name
    )


async def run(run: Run) -> None:
    assistant = agent(run)
    prompt = UserMsg(
        "user",
        [
            TextBlock(text=content.FILES),
            _data(run, "img.jpg", "image/jpeg"),
            _data(run, "task.pdf", "application/pdf"),
        ],
    )
    with run.trace():
        reply = await assistant.reply(prompt)
        print(reply.get_text_content())
