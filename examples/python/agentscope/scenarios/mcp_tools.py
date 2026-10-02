from agents import agent
from agentscope.mcp import MCPClient, StdioMCPConfig
from agentscope.message import UserMsg
from agentscope.tool import Toolkit

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    calculator = MCPClient(
        name="calculator",
        is_stateful=True,
        mcp_config=StdioMCPConfig(command=command, args=args),
    )
    with run.trace():
        await calculator.connect()
        try:
            assistant = agent(run, toolkit=Toolkit(mcps=[calculator]))
            reply = await assistant.reply(UserMsg("user", content.MCP))
            print(reply.get_text_content())
        finally:
            await calculator.close()
