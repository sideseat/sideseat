"""LlamaIndex model setup and sample execution."""

import asyncio
import importlib
import os
from typing import Any

from common.runner import create_trace_attributes, run_all_samples_base
from config import SAMPLES
from telemetry_setup import setup_telemetry


def get_model(model_id: str) -> Any:
    """Create the current LlamaIndex OpenAI-compatible LLM."""
    from llama_index.llms.openai import OpenAI

    return OpenAI(
        model=model_id,
        api_base=os.getenv("OPENAI_BASE_URL"),
        api_key=os.getenv("OPENAI_API_KEY", "fixture-key"),
        temperature=0,
        max_retries=0,
    )


def run_sample(name: str, args: Any) -> bool:
    """Run one LlamaIndex sample and flush its telemetry."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    owner = setup_telemetry(use_sideseat=args.sideseat)
    try:
        model = get_model(args.model)
        trace_attrs = create_trace_attributes("llamaindex", name)
        module = importlib.import_module(SAMPLES[name])
        asyncio.run(module.run(model, trace_attrs, owner))
        return True
    finally:
        shutdown = getattr(owner, "shutdown", None)
        if shutdown is not None:
            shutdown()


def run_all_samples(args: Any) -> None:
    """Run every LlamaIndex sample."""
    run_all_samples_base(SAMPLES, run_sample, args)
