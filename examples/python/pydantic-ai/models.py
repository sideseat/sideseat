"""Maps a harness model alias to a Pydantic AI model."""

from pydantic_ai.models.bedrock import BedrockConverseModel, BedrockModelSettings
from pydantic_ai.providers.bedrock import BedrockProvider

from harness import Model
from harness.models import region


def build(model: Model, *, reasoning: bool = False) -> BedrockConverseModel:
    if model.surface != "bedrock":
        raise SystemExit(
            f"the Pydantic AI suite runs Bedrock Converse models; {model.alias} is {model.surface}"
        )
    # Current Claude models think by default and omit the reasoning text; the reasoning scenario asks for a
    # summary so the telemetry carries visible reasoning.
    settings = BedrockModelSettings(max_tokens=16_000)
    if reasoning:
        settings["bedrock_additional_model_requests_fields"] = {
            "thinking": {"type": "adaptive", "display": "summarized"},
            "output_config": {"effort": "max"},
        }
    return BedrockConverseModel(
        model.id,
        provider=BedrockProvider(region_name=region()),
        settings=settings,
    )
