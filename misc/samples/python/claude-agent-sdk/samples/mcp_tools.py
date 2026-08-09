"""External stdio MCP server integration.

Demonstrates:
- mcp_servers with a stdio transport
- strict_mcp_config to ignore .mcp.json and user-level servers
- The mcp__<server>__<tool> naming convention required by allowed_tools
"""

import shutil
from pathlib import Path

from agent_helpers import build_options, print_stream, trace_scope
from claude_agent_sdk import query

QUERIES = [
    "Calculate an expression for me: What is 12345 plus 6789?",
    "What is 987 multiplied by 654? Use the calculator.",
]


async def run(model, trace_attrs: dict, client, env: dict):
    # Reuse the shared MCP calculator from misc/mcp, which has its own venv.
    mcp_server_dir = Path(__file__).parents[4] / "mcp"
    uv = shutil.which("uv") or "uv"

    options = build_options(
        model,
        env,
        mcp_servers={
            "calculator": {
                "type": "stdio",
                "command": uv,
                "args": ["run", "--directory", str(mcp_server_dir), "mcp-calculator"],
            }
        },
        # Ignore .mcp.json, user settings, and plugin servers so only ours loads.
        strict_mcp_config=True,
        allowed_tools=["mcp__calculator__calculate"],
        system_prompt="You help users calculate expressions. Always use the calculate tool.",
    )

    with trace_scope(client, "claude-agent-mcp-tools", trace_attrs):
        for prompt in QUERIES:
            print(f"--- {prompt} ---")
            await print_stream(query(prompt=prompt, options=options))
            print()
