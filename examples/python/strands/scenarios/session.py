from strands import Agent

from harness import Run, content


def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    for index, question in enumerate(content.SESSION, start=1):
        agent = Agent(
            model=run.llm, system_prompt=content.SYSTEM, callback_handler=None
        )
        with run.trace(f"session-turn-{index}"):
            print(agent(question))
