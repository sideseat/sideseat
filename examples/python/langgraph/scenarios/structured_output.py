from typing import Any, NotRequired

from langchain_core.messages import HumanMessage, SystemMessage
from langgraph.graph import END, START, MessagesState, StateGraph

from harness import Run, content


class PlanState(MessagesState):
    plan: NotRequired[content.TripPlan]


async def run(run: Run) -> None:
    # Bedrock's native structured output: forced tool choice, the function-calling method, is not
    # available on current Claude models.
    planner = run.llm.with_structured_output(content.TripPlan, method="json_schema")

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
