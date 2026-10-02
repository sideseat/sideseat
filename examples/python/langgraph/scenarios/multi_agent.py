from typing import Any

from agent import answer, build_agent
from langchain_core.messages import HumanMessage
from langgraph.graph import END, START, MessagesState, StateGraph
from tools import get_weather

from harness import Run, content


async def run(run: Run) -> None:
    researcher = build_agent(
        run.llm,
        tools=[get_weather],
        system="You research weather with the tool, then hand off to the writer.",
        name="researcher",
    )
    writer = build_agent(
        run.llm,
        system="You write the final packing list from the researcher's findings.",
        name="writer",
    )

    async def research(state: MessagesState) -> dict[str, Any]:
        result = await researcher.ainvoke({"messages": state["messages"]})
        return {"messages": [result["messages"][-1]]}

    async def write(state: MessagesState) -> dict[str, Any]:
        # The handoff: the writer sees the request and the researcher's findings, not the
        # researcher's tool traffic.
        handoff = HumanMessage(
            f"{state['messages'][0].text}\n\nThe researcher's findings:\n{state['messages'][-1].text}"
        )
        result = await writer.ainvoke({"messages": [handoff]})
        return {"messages": [result["messages"][-1]]}

    team = StateGraph(MessagesState)
    team.add_node("researcher", research)
    team.add_node("writer", write)
    team.add_edge(START, "researcher")
    team.add_edge("researcher", "writer")
    team.add_edge("writer", END)
    graph = team.compile(name="travel-team")
    with run.trace():
        print(
            answer(
                await graph.ainvoke({"messages": [HumanMessage(content.MULTI_AGENT)]})
            )
        )
