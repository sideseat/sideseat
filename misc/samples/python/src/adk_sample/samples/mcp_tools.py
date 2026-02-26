"""MCP server integration sample."""

import shutil
from pathlib import Path

from google.adk.agents import LlmAgent
from google.adk.runners import Runner
from google.adk.sessions import InMemorySessionService
from google.adk.tools.mcp_tool import McpToolset
from google.adk.tools.mcp_tool.mcp_session_manager import StdioConnectionParams
from google.genai import types
from mcp import StdioServerParameters

APP_NAME = "calculator_app"


async def run(model, trace_attrs: dict):
    """Run the mcp_tools sample."""
    # Use local MCP calculator server from misc/mcp (has its own venv with fastmcp)
    mcp_server_dir = Path(__file__).parents[5] / "mcp"
    uv = shutil.which("uv") or "uv"

    # Configure MCP toolset with stdio connection
    mcp_toolset = McpToolset(
        connection_params=StdioConnectionParams(
            server_params=StdioServerParameters(
                command=uv,
                args=["run", "--directory", str(mcp_server_dir), "mcp-calculator"],
            ),
        ),
    )

    agent = LlmAgent(
        model=model,
        name="calculator_assistant",
        instruction="You help users to calculate expressions.",
        tools=[mcp_toolset],
    )

    session_service = InMemorySessionService()
    session = await session_service.create_session(
        app_name=APP_NAME,
        user_id="demo-user",
        session_id=trace_attrs["session.id"],
    )

    runner = Runner(
        agent=agent,
        app_name=APP_NAME,
        session_service=session_service,
    )

    user_message = types.Content(
        role="user",
        parts=[types.Part(text="Calculate an expression for me: What is 12345 plus 6789?")],
    )

    async for event in runner.run_async(
        session_id=session.id,
        user_id="demo-user",
        new_message=user_message,
    ):
        if event.content and event.content.parts:
            for part in event.content.parts:
                if hasattr(part, "text") and part.text:
                    print(f"Result: {part.text}")
