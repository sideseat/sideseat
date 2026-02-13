"""Error sample — queries agent with nonexistent model ID to generate error telemetry."""

from agents import Agent, Runner
from opentelemetry import trace

INVALID_MODEL_ID = "nonexistent-model-id-12345"


def run(model_id: str, trace_attrs: dict, enable_thinking: bool = False):
    """Run the error sample with an invalid model ID."""
    tracer = trace.get_tracer(__name__)

    agent = Agent(
        name="Assistant",
        model=INVALID_MODEL_ID,
        instructions="You are a helpful assistant.",
    )

    with tracer.start_as_current_span("openai_agents.session", attributes=trace_attrs):
        Runner.run_sync(agent, "What is 2 + 2?")
