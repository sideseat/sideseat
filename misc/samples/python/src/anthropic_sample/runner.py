"""Sample runner with model and provider configuration."""

import importlib
from typing import Any, NamedTuple

from anthropic_sample.config import MODEL_ALIASES, SAMPLES
from anthropic_sample.telemetry_setup import setup_telemetry
from common.runner import create_trace_attributes, run_all_samples_base


class AnthropicModel(NamedTuple):
    """Anthropic client paired with a model ID."""

    client: Any
    model_id: str


def get_model(model_alias: str) -> AnthropicModel:
    """Create an Anthropic client with the resolved model ID."""
    from anthropic import Anthropic

    if model_alias in MODEL_ALIASES:
        _, model_id = MODEL_ALIASES[model_alias]
    else:
        model_id = model_alias

    client = Anthropic()
    return AnthropicModel(client=client, model_id=model_id)


def run_sample(name: str, args) -> bool:
    """Run a single sample with the specified configuration."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    print(f"Running sample: {name}")
    print(f"  Model: {args.model}")
    print(f"  SideSeat telemetry: {args.sideseat}")
    print()

    trace_attrs = create_trace_attributes("anthropic", name)
    client = setup_telemetry(use_sideseat=args.sideseat)

    model = get_model(args.model)
    module = importlib.import_module(SAMPLES[name])

    module.run(model, trace_attrs, client)
    return True


def run_all_samples(args):
    """Run all samples in sequence."""
    run_all_samples_base(SAMPLES, run_sample, args)
