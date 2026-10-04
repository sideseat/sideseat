from typing import Any

from agent import build_agent
from llama_index.tools.mcp import BasicMCPClient, McpToolSpec
from pydantic import BaseModel

from harness import Run, content
from harness.run import mcp_calculator_command


class _ToolSpec(McpToolSpec):
    """Leaves out parameters whose names begin with an underscore.

    The shared calculator accepts an optional `_meta` argument, and LlamaIndex builds a pydantic
    model per tool schema; pydantic refuses field names with a leading underscore, so the tool
    could not be listed at all.
    """

    def create_model_from_json_schema(
        self, schema: dict[str, Any], model_name: str = "DynamicModel"
    ) -> type[BaseModel]:
        properties = {
            name: value
            for name, value in schema.get("properties", {}).items()
            if not name.startswith("_")
        }
        return super().create_model_from_json_schema(
            {**schema, "properties": properties}, model_name
        )


async def run(run: Run) -> None:
    command, *args = mcp_calculator_command()
    calculator = _ToolSpec(client=BasicMCPClient(command, args=args))
    with run.trace():
        agent = build_agent(run.llm, tools=await calculator.to_tool_list_async())
        print(await agent.run(user_msg=content.MCP))
