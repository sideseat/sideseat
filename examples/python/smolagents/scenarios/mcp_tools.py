from mcp import StdioServerParameters
from smolagents import MCPClient, ToolCallingAgent

from harness import Run, content
from harness.run import mcp_calculator_command


def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    server = StdioServerParameters(command=command, args=args)
    with run.trace(), MCPClient(server, structured_output=True) as calculator:
        agent = ToolCallingAgent(
            tools=calculator, model=run.llm, instructions=content.SYSTEM
        )
        print(agent.run(content.MCP))
