"""Maps a harness model alias to a LlamaIndex Bedrock Converse LLM."""

from llama_index.llms.bedrock_converse import BedrockConverse

from harness import Model
from harness.models import region


def build(model: Model, *, reasoning: bool = False) -> BedrockConverse:
    if model.surface != "bedrock":
        raise SystemExit(
            f"the LlamaIndex suite runs Bedrock Converse models; {model.alias} is {model.surface}"
        )
    # Current Claude models think by default and omit the reasoning text; the reasoning scenario asks for a
    # summary so the telemetry carries visible reasoning. The fields go straight to Converse: the
    # `thinking` option validates against a schema that drops `display`.
    fields = (
        {
            "additionalModelRequestFields": {
                "thinking": {"type": "adaptive", "display": "summarized"},
                "output_config": {"effort": "max"},
            }
        }
        if reasoning
        else {}
    )
    return BedrockConverse(
        model=model.id,
        region_name=region(),
        max_tokens=16_000,
        additional_kwargs=fields,
    )
