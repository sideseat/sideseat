from semantic_kernel.agents import ChatCompletionAgent
from semantic_kernel.connectors.mcp import MCPStdioPlugin

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    with run.trace():
        async with MCPStdioPlugin(
            name="calculator", command=command, args=args
        ) as calculator:
            agent = ChatCompletionAgent(
                service=run.llm,
                name="assistant",
                instructions=content.SYSTEM,
                plugins=[calculator],
            )
            print(await agent.get_response(messages=content.MCP))
