"""Maps a harness model alias to a Semantic Kernel chat completion service."""

import boto3
from semantic_kernel.connectors.ai.bedrock import BedrockChatCompletion

from harness import Model
from harness.models import region


def build(model: Model) -> BedrockChatCompletion:
    if model.surface != "bedrock":
        raise SystemExit(
            f"the Semantic Kernel suite runs Bedrock Converse models; {model.alias} is {model.surface}"
        )
    return BedrockChatCompletion(
        model_id=model.id,
        runtime_client=boto3.client("bedrock-runtime", region_name=region()),
    )
