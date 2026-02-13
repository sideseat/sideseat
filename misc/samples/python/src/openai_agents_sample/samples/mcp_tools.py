"""MCP server integration sample."""

import sys
from pathlib import Path

from agents import Agent, Runner
from agents.mcp import MCPServerStdio
from opentelemetry import trace


def run(model_id: str, trace_attrs: dict, enable_thinking: bool = False):
    """Run the mcp_tools sample."""
    tracer = trace.get_tracer(__name__)

    # Use local MCP calculator server from misc/mcp
    mcp_server_path = Path(__file__).parents[5] / "mcp" / "calculator.py"

    async def run_with_mcp():
        async with MCPServerStdio(
            name="calculator",
            params={
                "command": sys.executable,
                "args": [str(mcp_server_path)],
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
