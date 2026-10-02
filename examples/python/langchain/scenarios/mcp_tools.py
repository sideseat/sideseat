from agent import converse
from langchain_core.messages import BaseMessage, HumanMessage, SystemMessage
from langchain_mcp_adapters.client import MultiServerMCPClient

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    client = MultiServerMCPClient(
        {"calculator": {"command": command, "args": args, "transport": "stdio"}}
    )
    messages: list[BaseMessage] = [
        SystemMessage(content.SYSTEM),
        HumanMessage(content.MCP),
    ]
    with run.trace():
        reply = await converse(run.llm, messages, tools=await client.get_tools())
        print(reply.text)
