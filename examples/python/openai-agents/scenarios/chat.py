from agents import Agent, Runner

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(name="assistant", instructions=content.SYSTEM, model=run.llm)
    with run.trace():
        result = await Runner.run(agent, content.CHAT)
        print(result.final_output)
