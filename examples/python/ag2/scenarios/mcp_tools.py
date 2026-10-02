from ag2.tools.toolkits import MCPStdioServerConfig, MCPToolkit
from agent import build_agent

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    calculator = MCPToolkit(MCPStdioServerConfig(command=command, args=args))
    agent = build_agent(run, tools=[calculator])
    with run.trace():
        print(await (await agent.ask(content.MCP)).content())
