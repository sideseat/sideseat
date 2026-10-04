"""The agent the scenarios run: a LangGraph state graph that loops between the model and its tools."""

from collections.abc import Sequence
from typing import Any

from langchain_core.language_models import BaseChatModel
from langchain_core.messages import SystemMessage
from langchain_core.tools import BaseTool
from langgraph.checkpoint.base import BaseCheckpointSaver
from langgraph.graph import END, START, MessagesState, StateGraph
from langgraph.prebuilt import ToolNode, tools_condition

from harness import content


def build_agent(
    llm: BaseChatModel,
    *,
    tools: Sequence[BaseTool] = (),
    system: str | None = content.SYSTEM,
    name: str = "agent",
    checkpointer: BaseCheckpointSaver[Any] | None = None,
) -> Any:
    model = llm.bind_tools(tools) if tools else llm
    frame = [SystemMessage(system)] if system else []

    async def call_model(state: MessagesState) -> dict[str, Any]:
        return {"messages": [await model.ainvoke([*frame, *state["messages"]])]}

    graph = StateGraph(MessagesState)
    graph.add_node("model", call_model)
    graph.add_edge(START, "model")
    if tools:
        # A tool's exception becomes an error result the model reads, rather than ending the run.
        graph.add_node("tools", ToolNode(tools, handle_tool_errors=True))
        graph.add_conditional_edges("model", tools_condition)
        graph.add_edge("tools", "model")
    else:
        graph.add_edge("model", END)
    return graph.compile(name=name, checkpointer=checkpointer)


def answer(state: dict[str, Any]) -> str:
    """The text of the last message in a final graph state."""
    return str(state["messages"][-1].text)
