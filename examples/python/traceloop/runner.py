"""TraceLoop model setup and sample execution."""

import importlib
import os
from typing import Any

from common.runner import create_trace_attributes, run_all_samples_base
from telemetry_setup import setup_telemetry

from config import SAMPLES


def get_client() -> Any:
    """Create the current OpenAI client against an explicit compatible endpoint."""
    from openai import OpenAI

    return OpenAI(
        api_key=os.getenv("OPENAI_API_KEY", "fixture-key"),
        base_url=os.getenv("OPENAI_BASE_URL", "http://127.0.0.1:5401/v1"),
        max_retries=0,
        timeout=30,
    )


def run_sample(name: str, args: Any) -> bool:
    """Run one TraceLoop sample and flush its telemetry."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    owner = setup_telemetry(use_sideseat=args.sideseat)
    try:
        trace_attrs = create_trace_attributes("traceloop", name)
        module = importlib.import_module(SAMPLES[name])
        module.run(get_client(), args.model, trace_attrs, owner)
        return True
    finally:
        shutdown = getattr(owner, "shutdown", None)
        if shutdown is not None:
            shutdown()


def run_all_samples(args: Any) -> None:
    """Run every TraceLoop sample."""
    run_all_samples_base(SAMPLES, run_sample, args)
