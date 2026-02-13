"""Sample runner with model and provider configuration."""

from autogen_sample.config import MODEL_ALIASES, REASONING_MODELS, SAMPLES
from autogen_sample.telemetry_setup import setup_telemetry
from common.models import DEFAULT_THINKING_BUDGET
from common.runner import (
    create_trace_attributes,
    run_all_samples_base,
    run_sample_module,
)


def get_model_client(model_alias: str, enable_thinking: bool = False):
    """Create model client from alias or full model ID.

    Args:
        model_alias: Model alias or full model ID
        enable_thinking: Enable extended thinking for supported models
    """
    # Resolve alias to (provider, model_id)
    if model_alias in MODEL_ALIASES:
        provider, model_id = MODEL_ALIASES[model_alias]
    else:
        # Treat as full model ID - infer provider from prefix
        if model_alias.startswith("openai-"):
            provider = "openai"
            model_id = model_alias[7:]
        elif model_alias.startswith("anthropic-"):
            provider = "anthropic"
            model_id = model_alias[10:]
        else:
            # Default to openai for backwards compatibility
            provider = "openai"
            model_id = model_alias

    # Check if thinking should be enabled for this model
    thinking_supported = model_alias in REASONING_MODELS
    use_thinking = enable_thinking and thinking_supported

    if provider == "openai":
        from autogen_ext.models.openai import OpenAIChatCompletionClient

        if use_thinking:
            print("  Extended thinking: enabled (reasoning_effort=medium)")
            return OpenAIChatCompletionClient(
                model=model_id,
                extra_create_args={"reasoning_effort": "medium"},
            )
        return OpenAIChatCompletionClient(model=model_id)

    elif provider == "anthropic":
        from autogen_core.models import ModelInfo
        from autogen_ext.models.anthropic import AnthropicChatCompletionClient

        # Anthropic models support function calling, but default model_info has it disabled.
        anthropic_model_info = ModelInfo(
            vision=True,
            function_calling=True,
            json_output=True,
            family="claude-4-sonnet",
            structured_output=True,
            multiple_system_messages=False,
        )

        if use_thinking:
            print(f"  Extended thinking: enabled (budget={DEFAULT_THINKING_BUDGET} tokens)")
            return AnthropicChatCompletionClient(
                model=model_id,
                model_info=anthropic_model_info,
                max_tokens=DEFAULT_THINKING_BUDGET + 8192,
                thinking={
                    "type": "enabled",
                    "budget_tokens": DEFAULT_THINKING_BUDGET,
                },
            )
        return AnthropicChatCompletionClient(model=model_id, model_info=anthropic_model_info)

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
    model_client = get_model_client(args.model, enable_thinking=enable_thinking)
    trace_attrs = create_trace_attributes("autogen", name)

    run_sample_module(SAMPLES[name], model_client, trace_attrs, is_async=True)
    return True


def run_all_samples(args):
    """Run all samples in sequence."""
    run_all_samples_base(SAMPLES, run_sample, args)
