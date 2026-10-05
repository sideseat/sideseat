from conversation import Conversation
from google.adk.agents import LlmAgent
from google.adk.tools.mcp_tool import McpToolset, StdioConnectionParams
from mcp import StdioServerParameters

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    calculator = McpToolset(
        connection_params=StdioConnectionParams(
            server_params=StdioServerParameters(command=command, args=args),
            # The first start of the calculator may install its environment.
            timeout=120,
        )
    )
    agent = LlmAgent(
        name="assistant",
        model=run.llm,
        instruction=content.SYSTEM,
        tools=[calculator],
    )
    try:
        with run.trace():
            print(await Conversation(run, agent).ask(content.MCP))
    finally:
        await calculator.close()
