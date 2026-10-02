from agno.agent import Agent
from agno.tools.mcp import MCPTools
from mcp import StdioServerParameters

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    server = StdioServerParameters(command=command, args=args)
    with run.trace():
        async with MCPTools(server_params=server, timeout_seconds=60) as calculator:
            agent = Agent(
                model=run.llm, instructions=content.SYSTEM, tools=[calculator]
            )
            result = await agent.arun(
                content.MCP, session_id=run.session_id, user_id=run.user_id
            )
            print(result.content)
