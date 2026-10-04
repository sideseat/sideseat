from agent import options, show
from claude_agent_sdk import query

from harness import Run, content


async def run(run: Run) -> None:
    schema = {"type": "json_schema", "schema": content.TripPlan.model_json_schema()}
    with run.trace():
        result = await show(
            query(prompt=content.STRUCTURED, options=options(run, output_format=schema))
        )
    print(content.TripPlan.model_validate(result.structured_output))
