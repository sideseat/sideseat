import json

from agent_framework import Agent

from harness import Run, content


async def run(run: Run) -> None:
    # Bedrock rejects the Anthropic native output format Agent Framework sends for response_format, so the
    # schema is given in the instructions and the answer is validated against it.
    schema = json.dumps(content.TripPlan.model_json_schema())
    agent = Agent(
        client=run.llm,
        name="assistant",
        instructions=(
            f"{content.SYSTEM} Answer with only a JSON object that matches this schema: {schema}"
        ),
    )
    session = agent.create_session(session_id=run.session_id)
    with run.trace():
        response = await agent.run(content.STRUCTURED, session=session)
        print(content.TripPlan.model_validate_json(response.text))
