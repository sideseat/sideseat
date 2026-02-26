"""Sample runner with model and provider configuration."""

import importlib
from typing import Any, NamedTuple

from common.runner import create_trace_attributes, run_all_samples_base
from openai_sample.config import MODEL_ALIASES, SAMPLES
from openai_sample.telemetry_setup import setup_telemetry


class OpenAIModel(NamedTuple):
    """OpenAI client paired with a model ID."""

    client: Any
    model_id: str


def get_model(model_alias: str) -> OpenAIModel:
    """Create an OpenAI client with the resolved model ID."""
    from openai import OpenAI

    if model_alias in MODEL_ALIASES:
        _, model_id = MODEL_ALIASES[model_alias]
    else:
        model_id = model_alias

    client = OpenAI()
    return OpenAIModel(client=client, model_id=model_id)


def run_sample(name: str, args) -> bool:
    """Run a single sample with the specified configuration."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    print(f"Running sample: {name}")
    print(f"  Model: {args.model}")
    print()

    trace_attrs = create_trace_attributes("openai", name)
    client = setup_telemetry()

    openai_model = get_model(args.model)
    module = importlib.import_module(SAMPLES[name])

    module.run(openai_model, trace_attrs, client)
    return True


def run_all_samples(args):
    """Run all samples in sequence."""
    run_all_samples_base(SAMPLES, run_sample, args)
