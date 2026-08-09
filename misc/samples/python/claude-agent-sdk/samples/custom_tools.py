"""In-process custom tools via an SDK MCP server.

Demonstrates:
- The @tool decorator with the simple type-mapping schema
- create_sdk_mcp_server, which runs in this process (no subprocess, no stdio)
- ToolAnnotations hints

Unlike samples/mcp_tools.py, no separate server process is spawned: the handlers
below execute inside the sample and are reached over an in-process transport.
"""

from typing import Any

from agent_helpers import build_options, print_stream, trace_scope
from claude_agent_sdk import ToolAnnotations, create_sdk_mcp_server, query, tool

INVENTORY = {"widget": 12, "gasket": 4, "flange": 0}

QUERIES = [
    "How many widgets and flanges are in stock?",
    "Restock flanges with 25 units, then tell me the new count.",
]


def _normalize(part: str) -> str:
    """Resolve a part name to an inventory key.

    The model naturally pluralizes ("widgets"), while the keys are singular, so
    match leniently instead of reporting a bogus "not a known part".
    """
    name = part.strip().lower()
    return name[:-1] if name not in INVENTORY and name.endswith("s") else name


@tool(
    "check_stock",
    "Look up the number of units in stock for a part.",
    {"part": str},
    annotations=ToolAnnotations(readOnlyHint=True),
)
async def check_stock(args: dict[str, Any]) -> dict[str, Any]:
    part = _normalize(args["part"])
    count = INVENTORY.get(part)
    text = f"{part}: not a known part" if count is None else f"{part}: {count} in stock"
    return {"content": [{"type": "text", "text": text}]}


@tool(
    "restock",
    "Add units to a part's stock level and return the new total.",
    {"part": str, "count": int},
)
async def restock(args: dict[str, Any]) -> dict[str, Any]:
    part, count = _normalize(args["part"]), args["count"]
    INVENTORY[part] = INVENTORY.get(part, 0) + count
    return {
        "content": [{"type": "text", "text": f"{part}: now {INVENTORY[part]} in stock"}]
    }


async def run(model, trace_attrs: dict, client, env: dict):
    inventory_server = create_sdk_mcp_server(
        name="inventory",
        version="1.0.0",
        tools=[check_stock, restock],
    )

    options = build_options(
        model,
        env,
        mcp_servers={"inventory": inventory_server},
        strict_mcp_config=True,
        allowed_tools=["mcp__inventory__check_stock", "mcp__inventory__restock"],
        # Without this the model sometimes shells out instead of calling the tools,
        # which defeats the point of the sample.
        disallowed_tools=["Bash"],
        system_prompt="You are a warehouse assistant. Use the inventory tools for all lookups.",
    )

    with trace_scope(client, "claude-agent-custom-tools", trace_attrs):
        for prompt in QUERIES:
            print(f"--- {prompt} ---")
            await print_stream(query(prompt=prompt, options=options))
            print()
