"""Sample runner with model configuration."""

import importlib

from common.runner import create_trace_attributes, run_all_samples_base
from openai_agents_sample.config import MODEL_ALIASES, REASONING_MODELS, SAMPLES
from openai_agents_sample.telemetry_setup import setup_telemetry


def get_model_id(model_alias: str) -> str:
    """Resolve model alias to model ID.

    Args:
        model_alias: Model alias or full model ID
    """
    if model_alias in MODEL_ALIASES:
        return MODEL_ALIASES[model_alias]
    # Strip openai- prefix if present
    if model_alias.startswith("openai-"):
        return model_alias[7:]
    return model_alias


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

    enable_thinking = name == "reasoning" and args.model in REASONING_MODELS
    model_id = get_model_id(args.model)
    trace_attrs = create_trace_attributes("openai-agents", name)

    # OpenAI Agents is sync
    module = importlib.import_module(SAMPLES[name])
    module.run(model_id, trace_attrs, enable_thinking=enable_thinking)
    return True


def run_all_samples(args):
    """Run all samples in sequence."""
    run_all_samples_base(SAMPLES, run_sample, args)
