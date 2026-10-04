from agent import options, show
from claude_agent_sdk import query
from tools import get_precipitation, get_weather, server

from harness import Run, content


async def run(run: Run) -> None:
    servers, allowed = server(get_weather, get_precipitation)
    settings = options(run, mcp_servers=servers, allowed_tools=allowed)
    with run.trace():
        await show(query(prompt=content.TOOL_USE, options=settings))
