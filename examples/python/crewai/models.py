"""Maps a harness model alias to a CrewAI LLM on its native Bedrock Converse provider."""

from typing import Any

from crewai import LLM

from harness import Model
from harness.models import region


def build(model: Model, *, reasoning: bool = False, stream: bool = False) -> LLM:
    if model.surface != "bedrock":
        raise SystemExit(
            f"the CrewAI suite runs Bedrock Converse models; {model.alias} is {model.surface}"
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
    # LLM hands provider options to the Bedrock provider it constructs, which its signature does not
    # declare. CrewAI ignores AWS_REGION, so the region is passed explicitly.
    options: dict[str, Any] = {
        "region_name": region(),
        "additional_model_request_fields": fields,
    }
    return LLM(model=f"bedrock/{model.id}", max_tokens=16_000, stream=stream, **options)
