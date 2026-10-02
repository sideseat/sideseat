"""Maps a harness model alias to an Agno model."""

import boto3
from agno.models.aws import Claude

from harness import Model
from harness.clients import bedrock_runtime_url
from harness.models import region


def build(model: Model, *, reasoning: bool = False) -> Claude:
    # Agno's Converse model (AwsBedrock) drops reasoning blocks; its Bedrock Claude model keeps them.
    if model.surface != "bedrock" or ".anthropic." not in model.id:
        raise SystemExit(
            f"the Agno suite runs Claude on Bedrock; {model.alias} is {model.surface}: {model.id}"
        )
    # Current Claude models think by default and omit the reasoning text; the reasoning scenario asks for a
    # summary so the telemetry carries visible reasoning.
    return Claude(
        id=model.id,
        session=boto3.Session(region_name=region()),
        client_params={"base_url": bedrock_runtime_url()},
        max_tokens=16_000,
        thinking={"type": "adaptive", "display": "summarized"} if reasoning else None,
        output_config={"effort": "max"} if reasoning else None,
    )
