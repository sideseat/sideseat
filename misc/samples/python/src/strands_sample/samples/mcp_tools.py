"""MCP server integration sample."""

import shutil
from pathlib import Path

from mcp import StdioServerParameters, stdio_client
from strands import Agent
from strands.tools.mcp import MCPClient


def run(model, trace_attrs: dict):
    """Run the mcp_tools sample."""
    # Use local MCP calculator server from misc/mcp (has its own venv with fastmcp)
    mcp_server_dir = Path(__file__).parents[5] / "mcp"
    uv = shutil.which("uv") or "uv"
    mcp_client = MCPClient(
        lambda: stdio_client(
            StdioServerParameters(
                command=uv,
                args=["run", "--directory", str(mcp_server_dir), "mcp-calculator"],
            )
        )
    )

    with mcp_client:
        tools = mcp_client.list_tools_sync()

        agent = Agent(
            model=model,
            tools=tools,
            system_prompt="You help users to calculate expressions.",
            trace_attributes=trace_attrs,
        )

        result = agent("Calculate an expression for me: What is 12345 plus 6789?")
        print(result)
