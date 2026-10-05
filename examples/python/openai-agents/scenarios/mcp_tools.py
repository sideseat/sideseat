from agents import Agent, Runner
from agents.mcp import MCPServerStdio

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    calculator = MCPServerStdio(
        params={"command": command, "args": args},
        name="calculator",
        # The first start of the calculator may install its environment.
        client_session_timeout_seconds=120,
    )
    with run.trace():
        async with calculator:
            agent = Agent(
                name="assistant",
                instructions=content.SYSTEM,
                model=run.llm,
                mcp_servers=[calculator],
            )
            result = await Runner.run(agent, content.MCP)
            print(result.final_output)
