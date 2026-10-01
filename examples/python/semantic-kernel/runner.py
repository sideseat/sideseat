"""Semantic Kernel model setup and sample execution."""

import asyncio
import importlib
import os
from typing import Any

from common.runner import create_trace_attributes, run_all_samples_base
from config import SAMPLES
from telemetry_setup import setup_telemetry


def get_service(model_id: str) -> Any:
    """Create the current Semantic Kernel OpenAI chat service."""
    from openai import AsyncOpenAI
    from semantic_kernel.connectors.ai.open_ai import OpenAIChatCompletion

    client = AsyncOpenAI(
        api_key=os.getenv("OPENAI_API_KEY", "fixture-key"),
        base_url=os.getenv("OPENAI_BASE_URL"),
        max_retries=0,
    )
    return OpenAIChatCompletion(
        ai_model_id=model_id,
        async_client=client,
    )


def run_sample(name: str, args: Any) -> bool:
    """Run one Semantic Kernel sample and flush its telemetry."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    owner = setup_telemetry(use_sideseat=args.sideseat)
    try:
        trace_attrs = create_trace_attributes("semantic-kernel", name)
        module = importlib.import_module(SAMPLES[name])
        asyncio.run(module.run(lambda: get_service(args.model), trace_attrs, owner))
        return True
    finally:
        shutdown = getattr(owner, "shutdown", None)
        if shutdown is not None:
            shutdown()


def run_all_samples(args: Any) -> None:
    """Run every Semantic Kernel sample."""
    run_all_samples_base(SAMPLES, run_sample, args)
