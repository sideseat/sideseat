from agents import Agent, Runner

from harness import Run, content


async def run(run: Run) -> None:
    # Two conversations, each its own trace, both attributed to one session and user.
    agent = Agent(name="assistant", instructions=content.SYSTEM, model=run.llm)
    for index, question in enumerate(content.SESSION, start=1):
        with run.trace(f"session-turn-{index}"):
            result = await Runner.run(agent, question)
            print(result.final_output)
