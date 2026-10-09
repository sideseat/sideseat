from agents import Agent, Runner, WebSearchTool

from harness import Run, content


async def run(run: Run) -> None:
    agent = Agent(
        name="assistant",
        instructions=content.SYSTEM,
        model=run.llm,
        tools=[WebSearchTool()],
    )
    with run.trace():
        result = await Runner.run(agent, content.SERVER_TOOLS)
        print(result.final_output)
