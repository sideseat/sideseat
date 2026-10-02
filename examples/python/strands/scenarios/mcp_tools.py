from mcp import StdioServerParameters, stdio_client
from strands import Agent
from strands.tools.mcp import MCPClient

from harness import Run, content
from harness.run import mcp_calculator_command


def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    calculator = MCPClient(
        lambda: stdio_client(StdioServerParameters(command=command, args=args))
    )
    with run.trace(), calculator:
        agent = Agent(
            model=run.llm,
            system_prompt=content.SYSTEM,
            tools=calculator.list_tools_sync(),
            callback_handler=None,
        )
        print(agent(content.MCP))
