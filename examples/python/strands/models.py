"""Maps a harness model alias to a Strands model."""

from strands.models import BedrockModel

from harness import Model
from harness.models import region


def build(model: Model, *, reasoning: bool = False) -> BedrockModel:
    if model.surface != "bedrock":
        raise SystemExit(
            f"the Strands suite runs Bedrock Converse models; {model.alias} is {model.surface}"
        )
    # Current Claude models think by default and omit the reasoning text; the reasoning scenario asks for a
    # summary so the telemetry carries visible reasoning.
    fields = (
        {
            "thinking": {"type": "adaptive", "display": "summarized"},
            "output_config": {"effort": "max"},
        }
        if reasoning
        else None
    )
    return BedrockModel(
        model_id=model.id,
        region_name=region(),
        max_tokens=16_000,
        additional_request_fields=fields,
    )
