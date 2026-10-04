import base64

from agents import agent
from conversation import Conversation

from harness import Run, content


def _data_url(media_type: str, data: bytes) -> str:
    return f"data:{media_type};base64,{base64.b64encode(data).decode()}"


async def run(run: Run) -> None:
    image = _data_url("image/jpeg", run.asset("img.jpg").read_bytes())
    pdf = _data_url("application/pdf", run.asset("task.pdf").read_bytes())
    attachments = [
        {"type": "input_image", "image_url": image},
        {"type": "input_file", "filename": "task.pdf", "file_data": pdf},
    ]
    assistant = agent(
        "travel-assistant", Conversation(run.llm, content.SYSTEM), attachments
    )
    with run.trace():
        print(await assistant(content.FILES))
