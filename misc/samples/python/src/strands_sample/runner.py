"""Sample runner with model and provider configuration."""

import importlib
import os

from common.models import DEFAULT_THINKING_BUDGET
from common.runner import create_trace_attributes, run_all_samples_base
from strands_sample.config import MODEL_ALIASES, REASONING_MODELS, SAMPLES
from strands_sample.telemetry_setup import setup_telemetry


def get_model(model_alias: str, enable_thinking: bool = False):
    """Create model instance from alias or full model ID.

    Model aliases embed the provider (e.g., bedrock-haiku, anthropic-sonnet).

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
            model_id = model_alias[8:]
        elif model_alias.startswith("anthropic-"):
            provider = "anthropic"
            model_id = model_alias[10:]
        elif model_alias.startswith("openai-"):
            provider = "openai"
            model_id = model_alias[7:]
        elif model_alias.startswith("gemini-"):
            provider = "gemini"
            model_id = model_alias[7:]
        else:
            provider = "bedrock"
            model_id = model_alias

    thinking_supported = model_alias in REASONING_MODELS
    use_thinking = enable_thinking and thinking_supported

    if provider == "bedrock":
        from strands.models import BedrockModel

        region = os.getenv("AWS_REGION") or os.getenv("AWS_DEFAULT_REGION", "us-east-1")
        print(f"  Region: {region}")

        if use_thinking:
            print(f"  Extended thinking: enabled (budget={DEFAULT_THINKING_BUDGET} tokens)")
            return BedrockModel(
                model_id=model_id,
                region_name=region,
                additional_request_fields={
                    "thinking": {
                        "type": "enabled",
                        "budget_tokens": DEFAULT_THINKING_BUDGET,
                    }
                },
            )
        return BedrockModel(model_id=model_id, region_name=region)

    elif provider == "openai":
        from strands.models.openai import OpenAIModel

        if use_thinking:
            print("  Extended thinking: enabled (reasoning_effort=medium)")
            return OpenAIModel(model_id=model_id, params={"reasoning_effort": "medium"})
        return OpenAIModel(model_id=model_id)

    elif provider == "anthropic":
        from strands.models.anthropic import AnthropicModel

        if use_thinking:
            print(f"  Extended thinking: enabled (budget={DEFAULT_THINKING_BUDGET} tokens)")
            return AnthropicModel(
                model_id=model_id,
                max_tokens=8192,
                params={
                    "thinking": {
                        "type": "enabled",
                        "budget_tokens": DEFAULT_THINKING_BUDGET,
                    }
                },
            )
        return AnthropicModel(model_id=model_id, max_tokens=8192)

    elif provider == "gemini":
        from strands.models.gemini import GeminiModel

        if use_thinking:
            print("  Extended thinking: not supported for Gemini")
        return GeminiModel(model_id=model_id)

    raise ValueError(f"Unknown provider: {provider}")


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
    trace_attrs = create_trace_attributes("strands", name)

    # Strands is sync
    module = importlib.import_module(SAMPLES[name])
    module.run(model, trace_attrs)
    return True


def run_all_samples(args):
    """Run all samples in sequence."""
    run_all_samples_base(SAMPLES, run_sample, args)
