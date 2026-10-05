import base64

from tools import converse, system

from harness import Run, content


def _data_url(media_type: str, data: bytes) -> str:
    return f"data:{media_type};base64,{base64.b64encode(data).decode()}"


def run(run: Run) -> None:
    image = _data_url("image/jpeg", run.asset("img.jpg").read_bytes())
    pdf = _data_url("application/pdf", run.asset("task.pdf").read_bytes())
    prompt = [
        {"type": "text", "text": content.FILES},
        {"type": "image_url", "image_url": {"url": image}},
        {"type": "file", "file": {"filename": "task.pdf", "file_data": pdf}},
    ]
    with run.trace():
        print(converse(run.llm, [system(), {"role": "user", "content": prompt}]))
