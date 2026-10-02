from smolagents import ToolCallingAgent

from harness import Run, content


def run(run: Run) -> None:
    agent = ToolCallingAgent(tools=[], model=run.llm, instructions=content.SYSTEM)
    with run.trace():
        print(agent.run(content.CHAT))
