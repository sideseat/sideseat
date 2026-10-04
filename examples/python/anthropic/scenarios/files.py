import base64

from tools import converse

from harness import Run, content


def _base64(name: str) -> str:
    return base64.b64encode(Run.asset(name).read_bytes()).decode("ascii")


def run(run: Run) -> None:
    prompt = [
        {"type": "text", "text": content.FILES},
        {
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/jpeg",
                "data": _base64("img.jpg"),
            },
        },
        {
            "type": "document",
            "source": {
                "type": "base64",
                "media_type": "application/pdf",
                "data": _base64("task.pdf"),
            },
        },
    ]
    with run.trace():
        print(converse(run.llm, [{"role": "user", "content": prompt}]))
