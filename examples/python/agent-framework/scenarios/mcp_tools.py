from agent_framework import Agent, MCPStdioTool

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    with run.trace():
        async with MCPStdioTool(
            name="calculator", command=command, args=args
        ) as calculator:
            agent = Agent(
                client=run.llm,
                name="assistant",
                instructions=content.SYSTEM,
                tools=[calculator],
            )
            session = agent.create_session(session_id=run.session_id)
            print((await agent.run(content.MCP, session=session)).text)
