"""MCP (Model Context Protocol) server integration sample.

Demonstrates:
- Connecting to an MCP server via stdio transport
- Loading tools from MCP server
- Using MCP tools in a ReAct agent
- Async agent execution with LangGraph
"""

import asyncio
import sys
from pathlib import Path

from langchain_core.messages import AIMessage, SystemMessage
from langchain_mcp_adapters.client import MultiServerMCPClient
from langgraph.prebuilt import create_react_agent


def extract_response(result: dict) -> str:
    """Extract the final text response from agent result.

    Args:
        result: The agent invoke result dictionary

    Returns:
        The extracted text response or error message
    """
    messages = result.get("messages", [])
    for msg in reversed(messages):
        if isinstance(msg, AIMessage) and msg.content:
            if isinstance(msg.content, str):
                return msg.content
            if isinstance(msg.content, list):
                for block in msg.content:
                    if isinstance(block, dict) and block.get("type") == "text":
                        return block.get("text", "")
    return "[No response generated]"


async def run_async(model, trace_attrs: dict):
    """Run the mcp_tools sample asynchronously.

    Args:
        model: LangChain chat model instance
        trace_attrs: Dictionary with session.id and user.id for tracing

    Raises:
        FileNotFoundError: If MCP server script not found
        RuntimeError: If MCP client fails to connect or get tools
    """
    # Locate MCP calculator server (5 levels up from this file)
    mcp_server_path = Path(__file__).parents[5] / "mcp" / "calculator.py"

    if not mcp_server_path.exists():
        raise FileNotFoundError(
            f"MCP calculator server not found at: {mcp_server_path}\n"
            "Ensure the misc/mcp/calculator.py file exists."
        )

    # Configure MCP client with stdio transport
    mcp_config = {
        "calculator": {
            "command": sys.executable,
            "args": [str(mcp_server_path)],
            "transport": "stdio",
        }
    }

    # Create client and get tools
    mcp_client = MultiServerMCPClient(mcp_config)

    try:
        tools = await mcp_client.get_tools()
    except Exception as e:
        raise RuntimeError(f"Failed to get tools from MCP server: {e}") from e

    if not tools:
        raise RuntimeError("MCP server returned no tools")

    print(f"Loaded {len(tools)} tool(s) from MCP server")

    # Create ReAct agent with MCP tools
    agent = create_react_agent(
        model=model,
        tools=tools,
        prompt=SystemMessage(content="You help users to calculate expressions."),
    )

    config = {
        "configurable": {"thread_id": trace_attrs["session.id"]},
        "metadata": {"user_id": trace_attrs["user.id"]},
    }

    try:
        result = await agent.ainvoke(
            {"messages": [("user", "Calculate an expression for me: What is 12345 plus 6789?")]},
            config=config,
        )
        print(extract_response(result))
    except Exception as e:
        print(f"[Error during agent execution: {e}]")


def run(model, trace_attrs: dict):
    """Run the mcp_tools sample.

    This sample demonstrates MCP (Model Context Protocol) integration:
    - Spawning a local MCP server process (calculator)
    - Discovering and loading tools from the server
    - Using MCP tools in a LangGraph ReAct agent

    Args:
        model: LangChain chat model instance
        trace_attrs: Dictionary with session.id and user.id for tracing
    """
    asyncio.run(run_async(model, trace_attrs))
