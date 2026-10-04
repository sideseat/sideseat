from agent import options, show
from claude_agent_sdk import query
from tools import book_flight, server

from harness import Run, content


async def run(run: Run) -> None:
    servers, allowed = server(book_flight)
    settings = options(run, mcp_servers=servers, allowed_tools=allowed)
    with run.trace():
        await show(query(prompt=content.ERROR, options=settings))
