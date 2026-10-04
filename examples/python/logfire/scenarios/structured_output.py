from agents import agent
from conversation import Conversation

from harness import Run, content


async def run(run: Run) -> None:
    schema = {"name": "TripPlan", "schema": content.TripPlan.model_json_schema()}
    output = {"text": {"format": {"type": "json_schema", **schema}}}
    planner = agent(
        "travel-assistant", Conversation(run.llm, content.SYSTEM, fields=output)
    )
    with run.trace():
        answer = await planner(content.STRUCTURED)
    print(content.TripPlan.model_validate_json(answer))
