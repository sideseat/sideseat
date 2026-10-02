from agent import answer, ask, build_agent
from haystack.dataclasses import ChatMessage
from haystack_integrations.tools.mcp import MCPToolset, StdioServerInfo

from harness import Run, content
from harness.run import mcp_calculator_command


def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    with run.trace():
        # The first start of the calculator may install its environment, hence the long timeout.
        calculator = MCPToolset(
            server_info=StdioServerInfo(command=command, args=args),
            connection_timeout=120,
        )
        try:
            agent = build_agent(run.llm, tools=[calculator])
            print(answer(ask(agent, ChatMessage.from_user(content.MCP))))
        finally:
            calculator.close()
