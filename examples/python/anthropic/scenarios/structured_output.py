from models import MAX_TOKENS

from harness import Run, content

# Bedrock rejects the Messages API's `output_config.format` for current Claude models, and a forced
# tool choice as well, so the schema is offered as a tool the model chooses to answer with.
PLAN_TOOL = {
    "name": "trip_plan",
    "description": "Return the finished trip plan.",
    "input_schema": content.TripPlan.model_json_schema(),
}


def run(run: Run) -> None:
    claude = run.llm
    with run.trace():
        response = claude.client.messages.create(
            model=claude.model,
            system=f"{content.SYSTEM} Answer by calling trip_plan.",
            max_tokens=MAX_TOKENS,
            messages=[{"role": "user", "content": content.STRUCTURED}],
            tools=[PLAN_TOOL],
        )
        call = next(block for block in response.content if block.type == "tool_use")
        print(content.TripPlan.model_validate(call.input))
