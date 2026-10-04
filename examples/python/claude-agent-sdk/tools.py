"""The shared example tools as an in-process MCP server, the Agent SDK's way to add tools."""

from __future__ import annotations

import json
from collections.abc import Awaitable, Callable
from typing import Any

from claude_agent_sdk import SdkMcpTool, create_sdk_mcp_server, tool

from harness import content

SERVER = "travel"

_CITY = {"type": "string", "description": "The city name."}
_SCHEMAS: dict[str, dict[str, Any]] = {
    "get_weather": {
        "type": "object",
        "properties": {
            "city": _CITY,
            "days": {
                "type": "integer",
                "description": "How many days to forecast, from 1 to 7.",
            },
        },
        "required": ["city"],
    },
    "get_precipitation": {
        "type": "object",
        "properties": {"city": _CITY},
        "required": ["city"],
    },
    "book_flight": {
        "type": "object",
        "properties": {
            "origin": {"type": "string", "description": "Departure city."},
            "destination": {"type": "string", "description": "Arrival city."},
            "date": {"type": "string", "description": "Travel date."},
        },
        "required": ["origin", "destination", "date"],
    },
}


def _wrap(function: Callable[..., object]) -> SdkMcpTool[Any]:
    name = function.__name__
    description = (function.__doc__ or name).strip().splitlines()[0]

    # A raised exception reaches the model as an error result, which the error scenario relies on.
    async def handler(args: dict[str, Any]) -> dict[str, Any]:
        result = function(**args)
        text = result if isinstance(result, str) else json.dumps(result)
        return {"content": [{"type": "text", "text": text}]}

    decorate: Callable[
        [Callable[[Any], Awaitable[dict[str, Any]]]], SdkMcpTool[Any]
    ] = tool(name, description, _SCHEMAS[name])
    return decorate(handler)


get_weather = _wrap(content.get_weather)
get_precipitation = _wrap(content.get_precipitation)
book_flight = _wrap(content.book_flight)


def server(*tools: SdkMcpTool[Any]) -> tuple[dict[str, Any], list[str]]:
    """``mcp_servers`` and ``allowed_tools`` values that expose ``tools`` to the agent."""
    config = create_sdk_mcp_server(SERVER, tools=list(tools))
    return {SERVER: config}, [f"mcp__{SERVER}__{t.name}" for t in tools]
