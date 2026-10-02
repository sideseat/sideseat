from fastmcp.client.transports import StdioTransport
from pydantic_ai import Agent
from pydantic_ai.mcp import MCPToolset

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    calculator = MCPToolset(StdioTransport(command=command, args=args))
    agent = Agent(run.llm, instructions=content.SYSTEM, toolsets=[calculator])
    with run.trace():
        async with agent:
            print((await agent.run(content.MCP)).output)
