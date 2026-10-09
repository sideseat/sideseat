"""Maps a harness model alias to a Smolagents model."""

from typing import Any

from smolagents import LiteLLMModel

from harness import Model, litellm_models
from harness.models import region


def build(model: Model, *, reasoning: bool = False) -> LiteLLMModel:
    # Smolagents' AmazonBedrockModel strips the tool configuration and cannot stream, so the suite reaches
    # Bedrock through LiteLLM, which Smolagents documents for every other provider.
    if not model.surface.startswith("bedrock"):
        raise SystemExit(
            f"the Smolagents suite runs Bedrock models; {model.alias} is {model.surface}"
        )
    extra: dict[str, Any] = {}
    if reasoning:
        # Current Claude models think by default and omit the reasoning text; the reasoning scenario asks
        # for a summary so the telemetry carries visible reasoning.
        extra["thinking"] = {"type": "adaptive", "display": "summarized"}
        extra["output_config"] = {"effort": "max"}
    litellm_models.pin()
    return LiteLLMModel(
        model_id=model.litellm_id,
        aws_region_name=region(),
        max_tokens=16_000,
        # ToolCallingAgent asks for a forced tool choice by default, which current Claude models reject
        # while thinking.
        tool_choice="auto",
        **extra,
    )
