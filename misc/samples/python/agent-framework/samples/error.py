"""Error sample — queries agent with nonexistent model ID to generate error telemetry."""

from agent_framework import Agent
from opentelemetry import trace

INVALID_MODEL_ID = "nonexistent-model-id-12345"


async def run(client, trace_attrs: dict):
    """Run the error sample with an invalid model ID.

    The model is overridden per call rather than by building a second client: the client the
    runner passes in already carries this suite's credentials, and constructing a bare
    OpenAIChatClient here needed an OPENAI_API_KEY the Bedrock-only setup does not have - so the
    sample failed on client construction and never produced the provider error it exists to
    record.
    """
    tracer = trace.get_tracer(__name__)

    agent = Agent(
        client=client,
        instructions="You are a helpful assistant.",
    )

    with tracer.start_as_current_span(
        "agent_framework.session", attributes=trace_attrs
    ):
        await agent.run("What is 2 + 2?", options={"model": INVALID_MODEL_ID})
