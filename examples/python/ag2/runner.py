"""AG2 model setup and sample execution."""

import asyncio
import importlib
import os
from functools import partial
from typing import Any

from common.runner import create_trace_attributes, run_all_samples_base
from telemetry_setup import setup_telemetry

from config import SAMPLES


def get_config(model_id: str, *, streaming: bool = False) -> Any:
    """Create the current AG2 OpenAI model configuration."""
    from ag2.config import OpenAIConfig

    return OpenAIConfig(
        model=model_id,
        api_key=os.getenv("OPENAI_API_KEY", "fixture-key"),
        base_url=os.getenv("OPENAI_BASE_URL"),
        max_retries=0,
        streaming=streaming,
    )


def run_sample(name: str, args: Any) -> bool:
    """Run one AG2 sample and flush its telemetry."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    owner = setup_telemetry(use_sideseat=args.sideseat)
    try:
        trace_attrs = create_trace_attributes("ag2", name)
        module = importlib.import_module(SAMPLES[name])
        config_factory = partial(get_config, args.model)
        asyncio.run(module.run(config_factory, trace_attrs, owner, args.sideseat))
        return True
    finally:
        shutdown = getattr(owner, "shutdown", None)
        if shutdown is not None:
            shutdown()


def run_all_samples(args: Any) -> None:
    """Run every AG2 sample."""
    run_all_samples_base(SAMPLES, run_sample, args)
