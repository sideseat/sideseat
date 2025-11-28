import uuid

import boto3
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from strands import Agent, tool, ToolContext
from strands.models import BedrockModel
from strands.telemetry import StrandsTelemetry
from strands.telemetry.config import get_otel_resource


MODEL_ID = "us.anthropic.claude-haiku-4-5-20251001-v1:0"
OTLP_ENDPOINT = "http://127.0.0.1:5001/otel/v1/traces"


@tool(context=True)
def weather_forecast(
    tool_context: ToolContext,
    city: str,
    days: int = 3,
) -> str:
    """Get weather forecast for a city.

    Args:
        city: The name of the city
        days: Number of days for the forecast
    """

    print(f"The agent name is {tool_context.agent.name}")
    print(f"Custom data: {tool_context.invocation_state.get('custom_data')}")

    return f"Weather forecast for {city} for the next {days} days is sunny."


def main():
    session = boto3.Session(region_name="us-east-1")
    bedrock_model = BedrockModel(model_id=MODEL_ID, boto_session=session)

    # Generate session ID for this run
    session_id = f"strands-demo-{uuid.uuid4().hex[:16]}"
    user_id = "demo-user"
    print(f"Session ID: {session_id}")

    # Create custom resource with session.id and user.id
    base_resource = get_otel_resource()
    custom_resource = base_resource.merge(
        Resource.create({
            "session.id": session_id,
            "user.id": user_id,
        })
    )

    # Create tracer provider with custom resource
    tracer_provider = TracerProvider(resource=custom_resource)

    # Setup telemetry with custom tracer provider
    telemetry = StrandsTelemetry(tracer_provider=tracer_provider)
    telemetry.setup_otlp_exporter(
        endpoint=OTLP_ENDPOINT,
        headers={"key1": "value1", "key2": "value2"},
    )

    agent = Agent(model=bedrock_model, tools=[weather_forecast])

    agent(
        "Provide a 3-day weather forecast for New York City and greet the user.",
        invocation_state={"custom_data": "You're the best agent ;)"},
    )


if __name__ == "__main__":
    main()
