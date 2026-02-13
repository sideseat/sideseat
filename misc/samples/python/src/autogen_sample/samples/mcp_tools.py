"""MCP server integration sample."""

import sys
from pathlib import Path

from autogen_agentchat.agents import AssistantAgent
from autogen_agentchat.conditions import MaxMessageTermination
from autogen_agentchat.teams import RoundRobinGroupChat
from autogen_ext.tools.mcp import StdioServerParams, mcp_server_tools
from opentelemetry import trace


async def run(model_client, trace_attrs: dict):
    """Run the mcp_tools sample."""
    tracer = trace.get_tracer(__name__)

    # Use local MCP calculator server from misc/mcp
    mcp_server_path = Path(__file__).parents[5] / "mcp" / "calculator.py"

    server_params = StdioServerParams(
        command=sys.executable,
        args=[str(mcp_server_path)],
    )

    # Get tools from MCP server
    tools = await mcp_server_tools(server_params)

    agent = AssistantAgent(
        name="calculator_assistant",
        model_client=model_client,
        tools=tools,
        system_message="You help users to calculate expressions.",
    )

    termination = MaxMessageTermination(max_messages=5)
    team = RoundRobinGroupChat([agent], termination_condition=termination)

    with tracer.start_as_current_span(
        "autogen.session",
        attributes=trace_attrs,
    ):
        result = await team.run(task="Calculate an expression for me: What is 12345 plus 6789?")

        for message in result.messages:
            if hasattr(message, "content") and message.content:
                if hasattr(message, "source") and message.source == agent.name:
                    print(f"Result: {message.content}")

    await model_client.close()
