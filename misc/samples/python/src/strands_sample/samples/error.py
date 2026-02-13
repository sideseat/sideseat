"""Error sample — queries agent with nonexistent model ID to generate error telemetry."""

from strands import Agent
from strands.models import BedrockModel

INVALID_MODEL_ID = "nonexistent-model-id-12345"


def run(model, trace_attrs: dict):
    """Run the error sample with an invalid model ID."""
    invalid_model = BedrockModel(model_id=INVALID_MODEL_ID)
    agent = Agent(
        model=invalid_model,
        system_prompt="You are a helpful assistant.",
        trace_attributes=trace_attrs,
    )
    agent("What is 2 + 2?")
