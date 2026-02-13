"""Sample runner with model and provider configuration.

This module provides:
- Model alias resolution and instantiation
- Provider-specific configuration (Bedrock, OpenAI, Anthropic, Gemini)
- Extended thinking setup for supported models
- Sample execution with telemetry
"""

import importlib
import os
import traceback

from common.models import DEFAULT_THINKING_BUDGET
from common.runner import create_trace_attributes
from langgraph_sample.config import MODEL_ALIASES, REASONING_MODELS, SAMPLES
from langgraph_sample.telemetry_setup import setup_telemetry


def get_model(model_alias: str, enable_thinking: bool = False):
    """Create LangChain chat model instance from alias or full model ID.

    Supports multiple providers:
    - bedrock: AWS Bedrock via ChatBedrockConverse
    - anthropic: Anthropic API via ChatAnthropic
    - openai: OpenAI API via ChatOpenAI
    - gemini: Google Gemini via ChatGoogleGenerativeAI

    Args:
        model_alias: Model alias (e.g., 'bedrock-haiku') or full model ID
        enable_thinking: Enable extended thinking for supported models

    Returns:
        Configured LangChain chat model instance

    Raises:
        ValueError: If provider is unknown
        ImportError: If provider's LangChain package is not installed
    """
    # Resolve alias to (provider, model_id)
    if model_alias in MODEL_ALIASES:
        provider, model_id = MODEL_ALIASES[model_alias]
    else:
        # Infer provider from prefix
        if model_alias.startswith("bedrock-"):
            provider = "bedrock"
            model_id = model_alias.removeprefix("bedrock-")
        elif model_alias.startswith("anthropic-"):
            provider = "anthropic"
            model_id = model_alias.removeprefix("anthropic-")
        elif model_alias.startswith("openai-"):
            provider = "openai"
            model_id = model_alias.removeprefix("openai-")
        elif model_alias.startswith("gemini-"):
            provider = "gemini"
            model_id = model_alias.removeprefix("gemini-")
        else:
            # Default to bedrock with full model_alias as model_id
            provider = "bedrock"
            model_id = model_alias

    thinking_supported = model_alias in REASONING_MODELS
    use_thinking = enable_thinking and thinking_supported

    if provider == "bedrock":
        from langchain_aws import ChatBedrockConverse

        region = os.getenv("AWS_REGION") or os.getenv("AWS_DEFAULT_REGION", "us-east-1")
        print(f"  Region: {region}")

        if use_thinking:
            print(f"  Extended thinking: enabled (budget={DEFAULT_THINKING_BUDGET} tokens)")
            return ChatBedrockConverse(
                model=model_id,
                region_name=region,
                additional_model_request_fields={
                    "thinking": {
                        "type": "enabled",
                        "budget_tokens": DEFAULT_THINKING_BUDGET,
                    }
                },
            )
        return ChatBedrockConverse(model=model_id, region_name=region)

    elif provider == "openai":
        from langchain_openai import ChatOpenAI

        if use_thinking:
            print("  Extended thinking: enabled (via reasoning_effort)")
        return ChatOpenAI(model=model_id)

    elif provider == "anthropic":
        from langchain_anthropic import ChatAnthropic

        if use_thinking:
            print(f"  Extended thinking: enabled (budget={DEFAULT_THINKING_BUDGET} tokens)")
            return ChatAnthropic(
                model=model_id,
                max_tokens=8192,
                thinking={
                    "type": "enabled",
                    "budget_tokens": DEFAULT_THINKING_BUDGET,
                },
            )
        return ChatAnthropic(model=model_id, max_tokens=8192)

    elif provider == "gemini":
        from langchain_google_genai import ChatGoogleGenerativeAI

        if use_thinking:
            print("  Extended thinking: not supported for Gemini")
        return ChatGoogleGenerativeAI(model=model_id)

    else:
        raise ValueError(f"Unknown provider: {provider}")


def run_sample(name: str, args):
    """Run a single sample with the specified configuration.

    Args:
        name: Sample name (must be in SAMPLES dict)
        args: Parsed CLI arguments with model and sideseat fields

    Returns:
        True if sample ran successfully, False otherwise
    """
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        print(f"Available samples: {', '.join(SAMPLES.keys())}")
        return False

    print(f"Running sample: {name}")
    print(f"  Model: {args.model}")
    print(f"  SideSeat telemetry: {args.sideseat}")
    print()

    # Setup telemetry
    setup_telemetry(use_sideseat=args.sideseat)

    # Enable extended thinking for reasoning sample
    enable_thinking = name == "reasoning"

    try:
        model = get_model(args.model, enable_thinking=enable_thinking)
    except ImportError as e:
        print(f"Missing dependency: {e}")
        print("Install the required package and try again.")
        return False
    except ValueError as e:
        print(f"Configuration error: {e}")
        return False

    trace_attrs = create_trace_attributes("langgraph", name)

    # Import and run sample
    try:
        module = importlib.import_module(SAMPLES[name])
    except ImportError as e:
        print(f"Failed to import sample module: {e}")
        return False

    try:
        module.run(model, trace_attrs)
        return True
    except Exception as e:
        print(f"Sample error: {e}")
        traceback.print_exc()
        return False


def run_all_samples(args):
    """Run all samples in sequence.

    Args:
        args: Parsed CLI arguments
    """
    results = []

    for name in SAMPLES:
        print(f"\n{'=' * 60}")
        print(f"Running: {name}")
        print(f"{'=' * 60}")

        try:
            success = run_sample(name, args)
            results.append((name, success, None))
        except Exception as e:
            results.append((name, False, str(e)))
            print(f"FAILED: {e}")
            traceback.print_exc()

    # Summary
    print(f"\n{'=' * 60}")
    print("Summary")
    print(f"{'=' * 60}")

    passed = sum(1 for _, success, _ in results if success)

    for name, success, error in results:
        status = "OK" if success else f"FAILED: {error or 'See above'}"
        print(f"  {name}: {status}")

    print(f"\nPassed: {passed}/{len(results)}")
