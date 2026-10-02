from agent import build_agent
from llama_index.tools.mcp import BasicMCPClient, McpToolSpec

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    calculator = McpToolSpec(client=BasicMCPClient(command, args=args))
    with run.trace():
        agent = build_agent(run.llm, tools=await calculator.to_tool_list_async())
        print(await agent.run(user_msg=content.MCP))
