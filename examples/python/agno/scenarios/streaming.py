from agno.agent import Agent
from agno.run.agent import RunContentEvent
from tools import get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(model=run.llm, instructions=content.SYSTEM, tools=[get_weather])
    with run.trace():
        for event in agent.run(
            content.STREAMING,
            stream=True,
            session_id=run.session_id,
            user_id=run.user_id,
        ):
            if isinstance(event, RunContentEvent) and isinstance(event.content, str):
                print(event.content, end="", flush=True)
        print()
