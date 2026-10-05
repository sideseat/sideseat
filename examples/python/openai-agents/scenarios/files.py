import base64
from typing import Any

from agents import Agent, Runner

from harness import Run, content


def _data_url(media_type: str, data: bytes) -> str:
    return f"data:{media_type};base64,{base64.b64encode(data).decode()}"


async def run(run: Run) -> None:
    agent = Agent(name="assistant", instructions=content.SYSTEM, model=run.llm)
    turn: list[Any] = [
        {
            "role": "user",
            "content": [
                {"type": "input_text", "text": content.FILES},
                {
                    "type": "input_image",
                    "image_url": _data_url(
                        "image/jpeg", run.asset("img.jpg").read_bytes()
                    ),
                    "detail": "auto",
                },
                {
                    "type": "input_file",
                    "filename": "task.pdf",
                    "file_data": _data_url(
                        "application/pdf", run.asset("task.pdf").read_bytes()
                    ),
                },
            ],
        }
    ]
    with run.trace():
        result = await Runner.run(agent, turn)
        print(result.final_output)
