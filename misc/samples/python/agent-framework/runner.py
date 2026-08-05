"""Sample runner with model and provider configuration."""

import asyncio

from config import MODEL_ALIASES, REASONING_MODELS, SAMPLES
from telemetry_setup import setup_telemetry
from common.models import DEFAULT_THINKING_BUDGET
from common.runner import create_trace_attributes, run_all_samples_base


def get_client(model_alias: str, enable_thinking: bool = False):
    """Create an Agent Framework client from alias or full model ID.

    agent-framework 1.15 renamed the constructor argument from `model_id` to `model` and
    removed `OpenAIResponsesClient`; reasoning is now requested per call rather than by
    picking a different client class.
    """
    if model_alias in MODEL_ALIASES:
        provider, model_id = MODEL_ALIASES[model_alias]
    else:
        if model_alias.startswith("openai-"):
            provider = "openai"
            model_id = model_alias[7:]
        elif model_alias.startswith("anthropic-"):
            provider = "anthropic"
            model_id = model_alias[10:]
        else:
            provider = "openai"
            model_id = model_alias

    thinking_supported = model_alias in REASONING_MODELS
    use_thinking = enable_thinking and thinking_supported
    if use_thinking:
        print("  Extended thinking: enabled")

    # bedrock-* aliases keep the suite runnable on AWS credentials alone.
    if model_alias.startswith("bedrock-"):
        if provider == "anthropic":
            # Native Bedrock client - it signs with the ambient AWS credentials itself,
            # so no SigV4 plumbing is needed on this path.
            from agent_framework.anthropic import AnthropicBedrockClient

            return AnthropicBedrockClient(model=model_id)

        # Bedrock's OpenAI-compatible endpoint has no native client, so the SigV4-signing
        # async client is injected instead.
        from agent_framework.openai import OpenAIChatClient

        from common.bedrock_openai import bedrock_async_openai_client

        return OpenAIChatClient(
            model=model_id, async_client=bedrock_async_openai_client()
        )

    if provider == "openai":
        from agent_framework.openai import OpenAIChatClient

        return OpenAIChatClient(model=model_id)

    if provider == "anthropic":
        from agent_framework.anthropic import AnthropicClient

        return AnthropicClient(model=model_id)

    raise ValueError(f"Unknown provider: {provider}")


def run_sample(name: str, args) -> bool:
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
    client = get_client(args.model, enable_thinking=enable_thinking)
    trace_attrs = create_trace_attributes("agent-framework", name)

    import importlib

    module = importlib.import_module(SAMPLES[name])

    extra_kwargs: dict = {}
    if name == "reasoning":
        from config import MODEL_ALIASES

        if args.model in MODEL_ALIASES:
            provider, _ = MODEL_ALIASES[args.model]
        else:
            provider = "openai" if args.model.startswith("openai-") else "anthropic"
        extra_kwargs["provider"] = provider

    asyncio.run(module.run(client, trace_attrs, **extra_kwargs))
    return True


def run_all_samples(args):
    """Run all samples in sequence."""
    run_all_samples_base(SAMPLES, run_sample, args)
