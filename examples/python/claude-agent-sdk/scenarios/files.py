import base64
from collections.abc import AsyncIterator
from typing import Any

from agent import options, show
from claude_agent_sdk import query

from harness import Run, content


def _block(kind: str, media_type: str, data: bytes) -> dict[str, Any]:
    source = {
        "type": "base64",
        "media_type": media_type,
        "data": base64.b64encode(data).decode(),
    }
    return {"type": kind, "source": source}


async def run(run: Run) -> None:
    # Content blocks reach the CLI only through streaming input: an iterable of user messages.
    message = {
        "type": "user",
        "message": {
            "role": "user",
            "content": [
                {"type": "text", "text": content.FILES},
                _block("image", "image/jpeg", run.asset("img.jpg").read_bytes()),
                _block(
                    "document", "application/pdf", run.asset("task.pdf").read_bytes()
                ),
            ],
        },
        "parent_tool_use_id": None,
    }

    async def prompt() -> AsyncIterator[dict[str, Any]]:
        yield message

    with run.trace():
        await show(query(prompt=prompt(), options=options(run, max_turns=1)))
