"""Maps a harness model alias to an ADK model: LiteLLM, which ADK documents for non-Gemini models."""

from typing import Any

from google.adk.models.lite_llm import LiteLlm

from harness import Model, litellm_models
from harness.models import region


def build(model: Model, *, reasoning: bool = False) -> LiteLlm:
    if model.surface not in {"bedrock", "bedrock-anthropic"}:
        raise SystemExit(
            f"the ADK suite runs Claude on Bedrock Converse; {model.alias} is {model.surface}"
        )
    extra: dict[str, Any] = {}
    if reasoning:
        # Current Claude models think by default and omit the reasoning text; the reasoning scenario asks
        # for a summary so the telemetry carries visible reasoning.
        extra["thinking"] = {"type": "adaptive", "display": "summarized"}
        extra["output_config"] = {"effort": "max"}
    litellm_models.pin()
    return LiteLlm(
        model=model.litellm_id,
        aws_region_name=region(),
        max_tokens=16_000,
        # Reasoning at maximum effort can take minutes; a shorter read timeout retries a request the
        # model already answered, and the retry records a second, different answer.
        timeout=600,
        **extra,
    )
