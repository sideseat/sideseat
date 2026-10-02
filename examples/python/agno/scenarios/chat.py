from agno.agent import Agent

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(model=run.llm, instructions=content.SYSTEM)
    with run.trace():
        result = agent.run(content.CHAT, session_id=run.session_id, user_id=run.user_id)
        print(result.content)
