from agent import options, show
from claude_agent_sdk import query

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    servers = {"calculator": {"type": "stdio", "command": command, "args": args}}
    settings = options(run, mcp_servers=servers, allowed_tools=["mcp__calculator"])
    with run.trace():
        await show(query(prompt=content.MCP, options=settings))
