"""Maps a harness model alias to an Agent Framework chat client."""

from collections.abc import Mapping
from typing import Any

from agent_framework.amazon import AnthropicBedrockClient
from agent_framework_anthropic._chat_client import BETA_FLAGS

from harness import Model
from harness.clients import anthropic_client


class BedrockClaudeClient(AnthropicBedrockClient):
    """Agent Framework's Anthropic Bedrock client, without the betas Bedrock does not offer.

    The client asks for the Anthropic API's MCP-client and code-execution betas on every request, and
    bedrock-runtime rejects a request that names either.
    """

    def _prepare_betas(self, options: Mapping[str, Any]) -> set[str]:
        return super()._prepare_betas(options) - set(BETA_FLAGS)


def build(model: Model) -> AnthropicBedrockClient:
    if model.surface != "bedrock-anthropic":
        raise SystemExit(
            f"the Agent Framework suite runs Claude through the Anthropic SDK; {model.alias} is {model.surface}"
        )
    return BedrockClaudeClient(
        model=model.id, anthropic_client=anthropic_client(model, asynchronous=True)
    )


#: Current Claude models think by default and omit the reasoning text; the reasoning scenario asks for a
#: summary so the telemetry carries visible reasoning.
REASONING_OPTIONS = {
    "max_tokens": 16_000,
    "thinking": {"type": "adaptive", "display": "summarized"},
    "output_config": {"effort": "max"},
}
