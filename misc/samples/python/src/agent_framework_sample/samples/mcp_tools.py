"""MCP server integration sample."""

import shutil
from pathlib import Path

from agent_framework import Agent, MCPStdioTool
from opentelemetry import trace


async def run(client, trace_attrs: dict):
    """Run the mcp_tools sample."""
    tracer = trace.get_tracer(__name__)

    # Use local MCP calculator server from misc/mcp (has its own venv with fastmcp)
    mcp_server_dir = Path(__file__).parents[5] / "mcp"
    uv = shutil.which("uv") or "uv"

    mcp_tool = MCPStdioTool(
        name="calculator",
        command=uv,
        args=["run", "--directory", str(mcp_server_dir), "mcp-calculator"],
        approval_mode="never_require",
    )

    agent = Agent(
        client=client,
        instructions="You help users to calculate expressions.",
        tools=[mcp_tool],
    )

    with tracer.start_as_current_span("agent_framework.session", attributes=trace_attrs):
        result = await agent.run("Calculate an expression for me: What is 12345 plus 6789?")
        print(f"Result: {result.text}")
