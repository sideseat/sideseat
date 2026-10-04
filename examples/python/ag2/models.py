"""Maps a harness model alias to an AG2 Bedrock Converse model config."""

from ag2.config.bedrock import BedrockConfig

from harness import Model
from harness.models import region


def build(
    model: Model, *, reasoning: bool = False, streaming: bool = False
) -> BedrockConfig:
    if model.surface != "bedrock":
        raise SystemExit(
            f"the AG2 suite runs Bedrock Converse models; {model.alias} is {model.surface}"
        )
    # Current Claude models think by default and omit the reasoning text; the reasoning scenario asks for a
    # summary so the telemetry carries visible reasoning.
    fields = (
        {
            "thinking": {"type": "adaptive", "display": "summarized"},
            "output_config": {"effort": "max"},
        }
        if reasoning
        else None
    )
    # BedrockConfig subclasses the ModelConfig protocol without its files client, which mypy reads
    # as abstract; the protocol is not enforced at runtime.
    return BedrockConfig(  # type: ignore[abstract]
        model=model.id,
        region_name=region(),
        max_tokens=16_000,
        # Reasoning at maximum effort can take minutes; a shorter read timeout retries a request the
        # model is still answering.
        timeout=600,
        streaming=streaming,
        additional_model_request_fields=fields,
    )
