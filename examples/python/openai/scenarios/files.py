import base64

from tools import chat, system

from harness import Run, content


def _data_url(media_type: str, data: bytes) -> str:
    return f"data:{media_type};base64,{base64.b64encode(data).decode()}"


def run(run: Run) -> None:
    prompt = [
        {"type": "text", "text": content.FILES},
        {
            "type": "image_url",
            "image_url": {
                "url": _data_url("image/jpeg", run.asset("img.jpg").read_bytes())
            },
        },
        {
            "type": "file",
            "file": {
                "filename": "task.pdf",
                "file_data": _data_url(
                    "application/pdf", run.asset("task.pdf").read_bytes()
                ),
            },
        },
    ]
    with run.trace():
        print(chat(run.llm, [system(), {"role": "user", "content": prompt}]))
