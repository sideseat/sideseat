"""MCP server integration sample."""

import shutil
from pathlib import Path

from agents import Agent, Runner
from agents.mcp import MCPServerStdio
from opentelemetry import trace


def run(model_id: str, trace_attrs: dict, enable_thinking: bool = False):
    """Run the mcp_tools sample."""
    tracer = trace.get_tracer(__name__)

    # Use local MCP calculator server from misc/mcp (has its own venv with fastmcp)
    mcp_server_dir = Path(__file__).parents[5] / "mcp"
    uv = shutil.which("uv") or "uv"

    async def run_with_mcp():
        async with MCPServerStdio(
            name="calculator",
            params={
                "command": uv,
                "args": ["run", "--directory", str(mcp_server_dir), "mcp-calculator"],
            },
        ) as server:
            agent = Agent(
                name="CalculatorAssistant",
                model=model_id,
                instructions="You help users to calculate expressions.",
                mcp_servers=[server],
            )

            with tracer.start_as_current_span(
                "openai_agents.session",
                attributes=trace_attrs,
            ):
                result = await Runner.run(
                    agent, "Calculate an expression for me: What is 12345 plus 6789?"
                )
                print(f"Result: {result.final_output}")

    import asyncio

    asyncio.run(run_with_mcp())
