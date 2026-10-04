from typing import Any

from haystack.components.agents import Agent
from haystack.dataclasses import ChatMessage
from haystack.tools import Tool

from harness import Run, content


def _plan(**fields: Any) -> str:
    return content.TripPlan.model_validate(fields).model_dump_json()


def run(run: Run) -> None:
    # Claude Sonnet 5.5 on Bedrock rejects Converse's native output format, so the schema is a tool
    # whose call ends the run: the agent's answer is the validated plan.
    plan = Tool(
        name="TripPlan",
        description=content.TripPlan.__doc__ or "A trip plan.",
        parameters=content.TripPlan.model_json_schema(),
        function=_plan,
    )
    agent = Agent(
        chat_generator=run.llm,
        tools=[plan],
        system_prompt=content.SYSTEM,
        exit_conditions=["TripPlan"],
    )
    with run.trace():
        result = agent.run(messages=[ChatMessage.from_user(content.STRUCTURED)])
        print(
            content.TripPlan.model_validate_json(
                result["last_message"].tool_call_result.result
            )
        )
