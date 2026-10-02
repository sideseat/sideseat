"""Maps a harness model alias to an AgentScope chat model."""

from agentscope.credential import AnthropicCredential
from agentscope.model import AnthropicChatModel
from pydantic import SecretStr

from harness import Model
from harness.clients import anthropic_client


def build(
    model: Model, *, reasoning: bool = False, stream: bool = False
) -> AnthropicChatModel:
    if model.surface not in ("bedrock-anthropic", "fake-anthropic"):
        raise SystemExit(
            f"the AgentScope suite runs Claude through the Anthropic SDK; {model.alias} is {model.surface}"
        )
    # Current Claude models think by default and omit the reasoning text; the reasoning scenario asks for a
    # summary so the telemetry carries visible reasoning.
    parameters = AnthropicChatModel.Parameters(
        max_tokens=16_000,
        thinking_mode="adaptive" if reasoning else None,
        thinking_display="summarized" if reasoning else None,
        reasoning_effort="max" if reasoning else None,
    )
    chat = AnthropicChatModel(
        credential=AnthropicCredential(api_key=SecretStr("unused")),
        model=model.id,
        parameters=parameters,
        stream=stream,
    )
    # AgentScope builds a first-party Anthropic client; the Anthropic SDK's Bedrock client serves the same
    # Messages API from bedrock-runtime with the ambient AWS credentials.
    chat.client = anthropic_client(model, asynchronous=True)
    return chat
