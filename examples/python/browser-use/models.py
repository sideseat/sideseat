"""Maps a harness model alias to a Browser Use chat model.

Browser Use asks for every step as a structured ``AgentOutput`` tool call. ``ChatAnthropicBedrock``
forces that tool, which current Claude models on Bedrock reject, and Laminar records
``ChatAWSBedrock``'s boto3 calls only with a separate instrumentation package. ``ChatAnthropic`` chooses
the tool automatically once adaptive thinking is on, so the suite runs it on an
``AsyncAnthropicBedrock`` client.
"""

from dataclasses import dataclass
from typing import Any

from anthropic import AsyncAnthropicBedrock
from browser_use import ChatAnthropic

from harness import Model
from harness.clients import bedrock_runtime_url
from harness.models import region


@dataclass
class ChatAnthropicOnBedrock(ChatAnthropic):
    def get_client(self) -> Any:
        # Long reasoning outlasts a shorter read timeout, and a retried call records a second answer.
        return AsyncAnthropicBedrock(
            aws_region=region(),
            base_url=bedrock_runtime_url(),
            timeout=600.0,
            max_retries=0,
        )


def build(model: Model) -> ChatAnthropicOnBedrock:
    if model.surface != "bedrock-anthropic":
        raise SystemExit(
            f"the Browser Use suite runs Claude through the Anthropic SDK; {model.alias} is {model.surface}"
        )
    return ChatAnthropicOnBedrock(
        model=model.id,
        max_tokens=16_000,
        thinking={"type": "adaptive"},
        max_retries=0,
    )
