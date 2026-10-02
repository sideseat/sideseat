from ag2 import DocumentInput, ImageInput
from agent import build_agent

from harness import Run, content


async def run(run: Run) -> None:
    agent = build_agent(run)
    with run.trace():
        # AG2's overloads type several inputs only alongside a response schema.
        reply = await agent.ask(  # type: ignore[call-overload]
            content.FILES,
            ImageInput(path=run.asset("img.jpg"), media_type="image/jpeg"),
            DocumentInput(path=run.asset("task.pdf"), media_type="application/pdf"),
        )
        print(await reply.content())
