"""Error sample — queries the selected provider with a nonexistent model ID."""

from strands import Agent

INVALID_MODEL_ID = "nonexistent-model-id-12345"


def run(model, trace_attrs: dict):
    """Run the error sample with an invalid model from the selected provider."""
    invalid_model = type(model)(model_id=INVALID_MODEL_ID)
    agent = Agent(
        model=invalid_model,
        system_prompt="You are a helpful assistant.",
        trace_attributes=trace_attrs,
    )
    agent("What is 2 + 2?")
