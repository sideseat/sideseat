from strands import Agent

from harness import Run, content


def run(run: Run) -> None:
    agent = Agent(model=run.llm, system_prompt=content.SYSTEM, callback_handler=None)
    with run.trace():
        for question in content.MULTI_TURN:
            print(agent(question))
