from typing import Any

from agents import agent
from converse import Conversation, Tool

from harness import Run, content

# Bedrock rejects Converse's `outputConfig.textFormat` for current Claude models, and a forced tool
# choice as well, so the schema is a tool the model chooses to hand its plan to.
PLAN = {
    "name": "trip_plan",
    "description": "Record the finished trip plan.",
    "inputSchema": {"json": content.TripPlan.model_json_schema()},
}


async def run(run: Run) -> None:
    plans: list[dict[str, Any]] = []

    def record(**plan: Any) -> str:
        plans.append(plan)
        return "Plan recorded."

    planner = agent(
        "travel-assistant",
        Conversation(
            run.llm,
            f"{content.SYSTEM} Hand the finished plan to trip_plan.",
            [Tool(PLAN, record)],
        ),
    )
    with run.trace():
        await planner(content.STRUCTURED)
    print(content.TripPlan.model_validate(plans[-1]))
