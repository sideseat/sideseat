from agno.agent import Agent
from agno.media import File, Image
from results import answer

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(model=run.llm, instructions=content.SYSTEM)
    image = Image(content=run.asset("img.jpg").read_bytes(), format="jpeg")
    document = File(
        content=run.asset("task.pdf").read_bytes(),
        format="pdf",
        mime_type="application/pdf",
        name="task",
    )
    with run.trace():
        result = agent.run(
            content.FILES,
            images=[image],
            files=[document],
            session_id=run.session_id,
            user_id=run.user_id,
        )
        print(answer(result))
