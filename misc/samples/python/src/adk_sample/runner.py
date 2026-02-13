"""Sample runner with model and provider configuration."""

import os

from google.adk.models.lite_llm import LiteLlm

from adk_sample.config import MODEL_ALIASES, REASONING_MODELS, SAMPLES
from adk_sample.telemetry_setup import setup_telemetry
from common.models import DEFAULT_THINKING_BUDGET
from common.runner import (
    create_trace_attributes,
    run_all_samples_base,
    run_sample_module,
)

AWS_REGION = os.getenv("AWS_REGION", os.getenv("AWS_DEFAULT_REGION", "us-east-1"))


def get_model(model_alias: str, enable_thinking: bool = False) -> LiteLlm:
    """Create model instance from alias or full model ID.

    Uses LiteLLM for non-Google model providers.

    Args:
        model_alias: Model alias or full model ID
        enable_thinking: Enable extended thinking for supported models
    """
    # Resolve alias to (provider, model_id)
    if model_alias in MODEL_ALIASES:
        provider, model_id = MODEL_ALIASES[model_alias]
    else:
        # Treat as full model ID - infer provider from prefix
        if model_alias.startswith("bedrock-"):
            provider = "bedrock"
            model_id = f"bedrock/{model_alias[8:]}"
        elif model_alias.startswith("anthropic-"):
            provider = "anthropic"
            model_id = f"anthropic/{model_alias[10:]}"
        elif model_alias.startswith("openai-"):
            provider = "openai"
            model_id = f"openai/{model_alias[7:]}"
        else:
            provider = "bedrock"
            model_id = f"bedrock/{model_alias}"

    thinking_supported = model_alias in REASONING_MODELS
    use_thinking = enable_thinking and thinking_supported

    # For Bedrock, set AWS region
    if provider == "bedrock":
        os.environ["AWS_REGION_NAME"] = AWS_REGION
        print(f"  Region: {AWS_REGION}")

    if use_thinking:
        if provider == "openai":
            print("  Extended thinking: enabled (reasoning_effort=medium)")
            return LiteLlm(model=model_id, reasoning_effort="medium")
        else:
            print(f"  Extended thinking: enabled (budget={DEFAULT_THINKING_BUDGET} tokens)")
            return LiteLlm(
                model=model_id,
                thinking={
                    "type": "enabled",
                    "budget_tokens": DEFAULT_THINKING_BUDGET,
                },
            )

    return LiteLlm(model=model_id)


def run_sample(name: str, args):
    """Run a single sample with the specified configuration."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    print(f"Running sample: {name}")
    print(f"  Model: {args.model}")
    print(f"  SideSeat telemetry: {args.sideseat}")
    print()

    setup_telemetry(use_sideseat=args.sideseat)

    enable_thinking = name == "reasoning"
    model = get_model(args.model, enable_thinking=enable_thinking)
    trace_attrs = create_trace_attributes("adk", name)

    run_sample_module(SAMPLES[name], model, trace_attrs, is_async=True)
    return True


def run_all_samples(args):
    """Run all samples in sequence."""
    run_all_samples_base(SAMPLES, run_sample, args)
