from typing import Any

from agents import agent
from converse import Conversation, Tool
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

from harness import Run, content
from harness.run import mcp_calculator_command


def _tool(session: ClientSession, spec: Any) -> Tool:
    # TraceLoop's MCP instrumentation traces the call itself.
    async def call(**arguments: Any) -> str:
        result = await session.call_tool(spec.name, arguments)
        return "".join(getattr(block, "text", "") for block in result.content)

    converse_spec = {
        "name": spec.name,
        "description": spec.description or spec.name,
        "inputSchema": {"json": spec.inputSchema},
    }
    return Tool(converse_spec, call)


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    server = StdioServerParameters(command=command, args=args)
    with run.trace():
        async with (
            stdio_client(server) as (read, write),
            ClientSession(read, write) as session,
        ):
            await session.initialize()
            tools = [
                _tool(session, spec) for spec in (await session.list_tools()).tools
            ]
            assistant = agent(
                "travel-assistant", Conversation(run.llm, content.SYSTEM, tools)
            )
            print(await assistant(content.MCP))
