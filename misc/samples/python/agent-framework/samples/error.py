"""Error sample — queries agent with nonexistent model ID to generate error telemetry."""

from agent_framework import Agent
from agent_framework.openai import OpenAIChatClient
from opentelemetry import trace

INVALID_MODEL_ID = "nonexistent-model-id-12345"


async def run(client, trace_attrs: dict):
    """Run the error sample with an invalid model ID."""
    tracer = trace.get_tracer(__name__)

    # Always use an invalid OpenAI client regardless of the passed client
    invalid_client = OpenAIChatClient(model_id=INVALID_MODEL_ID)

    agent = Agent(
        client=invalid_client,
        instructions="You are a helpful assistant.",
    )

    with tracer.start_as_current_span(
        "agent_framework.session", attributes=trace_attrs
    ):
        await agent.run("What is 2 + 2?")
