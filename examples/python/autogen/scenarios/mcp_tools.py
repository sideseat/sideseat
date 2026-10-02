from agent import answer
from autogen_agentchat.agents import AssistantAgent
from autogen_ext.tools.mcp import McpWorkbench, StdioServerParams

from harness import Run, content
from harness.run import mcp_calculator_command


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    with run.trace():
        async with McpWorkbench(
            StdioServerParams(command=command, args=args)
        ) as calculator:
            agent = AssistantAgent(
                "assistant",
                model_client=run.llm,
                system_message=content.SYSTEM,
                workbench=calculator,
                reflect_on_tool_use=True,
                max_tool_iterations=5,
            )
            print(answer(await agent.run(task=content.MCP)))
