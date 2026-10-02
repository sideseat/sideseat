from smolagents import ToolCallingAgent

from harness import Run, content


def run(run: Run) -> None:
    agent = ToolCallingAgent(tools=[], model=run.llm, instructions=content.SYSTEM)
    with run.trace():
        for index, question in enumerate(content.MULTI_TURN):
            # reset=False keeps the agent's memory, so each run re-sends the earlier turns.
            print(agent.run(question, reset=index == 0))
