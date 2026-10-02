from agent import ask, build_agent
from haystack.dataclasses import ChatMessage
from models import build

from harness import Run, content


def run(run: Run) -> None:
    # Bedrock's native structured output requires every object to forbid additional properties.
    schema = {**content.TripPlan.model_json_schema(), "additionalProperties": False}
    llm = build(run.model, response_format={"name": "TripPlan", "schema": schema})
    agent = build_agent(llm)
    with run.trace():
        result = ask(agent, ChatMessage.from_user(content.STRUCTURED))
        print(
            content.TripPlan.model_validate(
                result["last_message"].meta["structured_output"]
            )
        )
