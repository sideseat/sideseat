"""Sample runner with model and provider configuration."""

import asyncio

from agent_framework_sample.config import MODEL_ALIASES, REASONING_MODELS, SAMPLES
from agent_framework_sample.telemetry_setup import setup_telemetry
from common.models import DEFAULT_THINKING_BUDGET
from common.runner import create_trace_attributes, run_all_samples_base


def get_client(model_alias: str, enable_thinking: bool = False):
    """Create an Agent Framework client from alias or full model ID.

    Returns an OpenAIChatClient, OpenAIResponsesClient (for reasoning), or
    AnthropicClient depending on the provider.
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

    if provider == "openai":
        # Use OpenAIResponsesClient for reasoning (supports reasoning_effort)
        # Use OpenAIChatClient for standard tasks
        if use_thinking:
            from agent_framework.openai import OpenAIResponsesClient

            print("  Extended thinking: enabled (reasoning_effort=medium)")
            return OpenAIResponsesClient(model_id=model_id)
        else:
            from agent_framework.openai import OpenAIChatClient

            return OpenAIChatClient(model_id=model_id)

    elif provider == "anthropic":
        from agent_framework.anthropic import AnthropicClient

        if use_thinking:
            print(f"  Extended thinking: enabled (budget={DEFAULT_THINKING_BUDGET} tokens)")
        return AnthropicClient(model_id=model_id)

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
        from agent_framework_sample.config import MODEL_ALIASES

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
