"""Maps a harness model alias to an Agent Framework chat client."""

from agent_framework.amazon import AnthropicBedrockClient

from harness import Model
from harness.clients import anthropic_client


def build(model: Model) -> AnthropicBedrockClient:
    if model.surface != "bedrock-anthropic":
        raise SystemExit(
            f"the Agent Framework suite runs Claude through the Anthropic SDK; {model.alias} is {model.surface}"
        )
    return AnthropicBedrockClient(
        model=model.id, anthropic_client=anthropic_client(model, asynchronous=True)
    )


#: Current Claude models think by default and omit the reasoning text; the reasoning scenario asks for a
#: summary so the telemetry carries visible reasoning.
REASONING_OPTIONS = {
    "max_tokens": 16_000,
    "thinking": {"type": "adaptive", "display": "summarized"},
    "output_config": {"effort": "max"},
}
