from agno.agent import Agent
from tools import get_precipitation, get_weather

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(
        model=run.llm,
        instructions=content.SYSTEM,
        tools=[get_weather, get_precipitation],
    )
    with run.trace():
        result = agent.run(
            content.TOOL_USE, session_id=run.session_id, user_id=run.user_id
        )
        print(result.content)
