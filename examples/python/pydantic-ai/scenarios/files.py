from pydantic_ai import Agent, BinaryContent

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(run.llm, instructions=content.SYSTEM)
    prompt = [
        content.FILES,
        BinaryContent(data=run.asset("img.jpg").read_bytes(), media_type="image/jpeg"),
        BinaryContent(
            data=run.asset("task.pdf").read_bytes(),
            media_type="application/pdf",
            identifier="task",
        ),
    ]
    with run.trace():
        print(agent.run_sync(prompt).output)
