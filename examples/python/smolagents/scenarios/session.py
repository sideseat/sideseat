from smolagents import ToolCallingAgent

from harness import Run, content


def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        agent = ToolCallingAgent(tools=[], model=run.llm, instructions=content.SYSTEM)
        with run.trace(f"session-turn-{index}"):
            print(agent.run(question))
