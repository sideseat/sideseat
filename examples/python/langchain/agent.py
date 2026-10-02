"""The tool-calling loop the scenarios run, built from LangChain's chat model and tool primitives."""

from collections.abc import Sequence
from typing import Any

from langchain_core.language_models import BaseChatModel
from langchain_core.messages import (
    AIMessage,
    AIMessageChunk,
    BaseMessage,
    ToolCall,
    ToolMessage,
)
from langchain_core.tools import BaseTool


async def converse(
    llm: BaseChatModel,
    messages: list[BaseMessage],
    *,
    tools: Sequence[BaseTool] = (),
    stream: bool = False,
) -> AIMessage:
    """Calls the model until it answers without a tool call, appending every turn to ``messages``."""
    model: Any = llm.bind_tools(tools) if tools else llm
    by_name = {tool.name: tool for tool in tools}
    while True:
        reply = (
            await _stream(model, messages) if stream else await model.ainvoke(messages)
        )
        messages.append(reply)
        if not reply.tool_calls:
            return reply
        for call in reply.tool_calls:
            messages.append(await _execute(by_name[call["name"]], call))


async def _stream(model: Any, messages: list[BaseMessage]) -> AIMessage:
    reply: AIMessageChunk | None = None
    async for chunk in model.astream(messages):
        print(chunk.text, end="", flush=True)
        reply = chunk if reply is None else reply + chunk
    print()
    assert reply is not None
    return AIMessage(
        content=reply.content,
        tool_calls=reply.tool_calls,
        response_metadata=reply.response_metadata,
        usage_metadata=reply.usage_metadata,
        id=reply.id,
    )


async def _execute(tool: BaseTool, call: ToolCall) -> ToolMessage:
    try:
        result: ToolMessage = await tool.ainvoke(call)
        return result
    except Exception as error:
        # The model reads the failure as the tool's result, the way LangGraph's ToolNode reports it.
        return ToolMessage(
            f"Error: {error!r}",
            tool_call_id=call["id"],
            name=call["name"],
            status="error",
        )
