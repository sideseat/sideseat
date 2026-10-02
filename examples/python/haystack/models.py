"""Maps a harness model alias to a Haystack chat generator on Bedrock Converse."""

from typing import Any

from haystack.components.generators.utils import print_streaming_chunk
from haystack_integrations.components.generators.amazon_bedrock import (
    AmazonBedrockChatGenerator,
)

from harness import Model
from harness.models import region


def build(
    model: Model,
    *,
    reasoning: bool = False,
    streaming: bool = False,
    response_format: dict[str, Any] | None = None,
) -> AmazonBedrockChatGenerator:
    if model.surface != "bedrock":
        raise SystemExit(
            f"the Haystack suite runs Bedrock Converse models; {model.alias} is {model.surface}"
        )
    generation: dict[str, Any] = {"maxTokens": 16_000}
    if reasoning:
        # Current Claude models think by default and omit the reasoning text; the reasoning
        # scenario asks for a summary so the telemetry carries visible reasoning.
        generation["thinking"] = {"type": "adaptive", "display": "summarized"}
        generation["output_config"] = {"effort": "max"}
    if response_format is not None:
        generation["response_format"] = response_format
    # The generator's default region comes from AWS_DEFAULT_REGION, not AWS_REGION.
    return AmazonBedrockChatGenerator(
        model=model.id,
        aws_region_name=region(),
        generation_kwargs=generation,
        streaming_callback=print_streaming_chunk if streaming else None,
    )
