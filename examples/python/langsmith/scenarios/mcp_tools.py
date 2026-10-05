from agent import answer, build_agent
from langchain_core.messages import HumanMessage
from langchain_mcp_adapters.client import MultiServerMCPClient

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    client = MultiServerMCPClient(
        {"calculator": {"command": command, "args": args, "transport": "stdio"}}
    )
    with run.trace():
        agent = build_agent(run.llm, tools=await client.get_tools())
        print(answer(await agent.ainvoke({"messages": [HumanMessage(content.MCP)]})))
