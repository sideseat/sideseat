"""Sample runner with model and provider configuration."""

import importlib
import os
from typing import Any, NamedTuple

from bedrock_sample.config import MODEL_ALIASES, SAMPLES
from bedrock_sample.telemetry_setup import setup_telemetry
from common.runner import create_trace_attributes, run_all_samples_base


class BedrockModel(NamedTuple):
    """Boto3 Bedrock client paired with a model ID."""

    client: Any
    model_id: str


def get_model(model_alias: str) -> BedrockModel:
    """Create a boto3 bedrock-runtime client with the resolved model ID."""
    import boto3

    if model_alias in MODEL_ALIASES:
        _, model_id = MODEL_ALIASES[model_alias]
    else:
        model_id = model_alias

    region = os.getenv("AWS_REGION") or os.getenv("AWS_DEFAULT_REGION", "us-east-1")
    print(f"  Region: {region}")

    client = boto3.client("bedrock-runtime", region_name=region)
    return BedrockModel(client=client, model_id=model_id)


def run_sample(name: str, args) -> bool:
    """Run a single sample with the specified configuration."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    print(f"Running sample: {name}")
    print(f"  Model: {args.model}")
    print()

    trace_attrs = create_trace_attributes("bedrock", name)
    client = setup_telemetry()

    bedrock = get_model(args.model)
    module = importlib.import_module(SAMPLES[name])

    module.run(bedrock, trace_attrs, client)
    return True


def run_all_samples(args):
    """Run all samples in sequence."""
    run_all_samples_base(SAMPLES, run_sample, args)
