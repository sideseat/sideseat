"""Maps a harness model alias to an AutoGen chat client for Claude on Bedrock."""

from autogen_core.models import ModelFamily, ModelInfo
from autogen_ext.models.anthropic import BaseAnthropicChatCompletionClient

from harness import Model
from harness.clients import anthropic_client


def build(
    model: Model, *, reasoning: bool = False
) -> BaseAnthropicChatCompletionClient:
    if model.surface != "bedrock-anthropic":
        raise SystemExit(
            f"the AutoGen suite runs Claude through the Anthropic SDK; {model.alias} is {model.surface}"
        )
    create_args: dict[str, object] = {"model": model.id, "max_tokens": 16_000}
    if reasoning:
        # Current Claude models think by default and omit the reasoning text; the reasoning
        # scenario asks for a summary so the telemetry carries visible reasoning.
        create_args["thinking"] = {"type": "adaptive", "display": "summarized"}
    # The base client takes the Anthropic SDK client it is given: AnthropicBedrock, built by the
    # harness so it reaches bedrock-runtime, or the capture proxy in front of it.
    return BaseAnthropicChatCompletionClient(
        client=anthropic_client(model, asynchronous=True),
        create_args=create_args,
        model_info=ModelInfo(
            vision=True,
            function_calling=True,
            json_output=False,
            structured_output=False,
            family=ModelFamily.UNKNOWN,
            multiple_system_messages=False,
        ),
    )
