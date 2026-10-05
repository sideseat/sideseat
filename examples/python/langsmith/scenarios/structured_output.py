from typing import Any, NotRequired

from langchain_core.messages import HumanMessage, SystemMessage
from langgraph.graph import END, START, MessagesState, StateGraph

from harness import Run, content


class PlanState(MessagesState):
    plan: NotRequired[content.TripPlan]


async def run(run: Run) -> None:
    # The schema is offered as a tool. Current Claude models reject both forced tool choice and
    # Bedrock's native output format, so the tool is offered with automatic choice.
    planner = run.llm.with_structured_output(content.TripPlan)

    async def plan(state: PlanState) -> dict[str, Any]:
        return {
            "plan": await planner.ainvoke(
                [SystemMessage(content.SYSTEM), *state["messages"]]
            )
        }

    graph = StateGraph(PlanState)
    graph.add_node("plan", plan)
    graph.add_edge(START, "plan")
    graph.add_edge("plan", END)
    agent = graph.compile(name="planner")
    with run.trace():
        state = await agent.ainvoke({"messages": [HumanMessage(content.STRUCTURED)]})
        print(state["plan"])
