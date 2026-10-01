"""Haystack model setup and sample execution."""

import importlib
import os
from typing import Any

from common.runner import create_trace_attributes, run_all_samples_base
from telemetry_setup import setup_telemetry

from config import SAMPLES


def get_model(model_id: str) -> Any:
    """Create the current Haystack OpenAI-compatible chat generator."""
    from haystack.components.generators.chat import OpenAIChatGenerator
    from haystack.utils import Secret

    return OpenAIChatGenerator(
        api_key=Secret.from_token(os.getenv("OPENAI_API_KEY", "fixture-key")),
        api_base_url=os.getenv("OPENAI_BASE_URL"),
        model=model_id,
        generation_kwargs={"temperature": 0},
        max_retries=0,
    )


def run_sample(name: str, args: Any) -> bool:
    """Run one Haystack sample and flush its telemetry."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    owner = setup_telemetry(use_sideseat=args.sideseat)
    try:
        trace_attrs = create_trace_attributes("haystack", name)
        module = importlib.import_module(SAMPLES[name])
        module.run(lambda: get_model(args.model), trace_attrs, owner)
        return True
    finally:
        shutdown = getattr(owner, "shutdown", None)
        if shutdown is not None:
            shutdown()


def run_all_samples(args: Any) -> None:
    """Run every Haystack sample."""
    run_all_samples_base(SAMPLES, run_sample, args)
