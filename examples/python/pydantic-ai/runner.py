"""Pydantic AI sample runner."""

import importlib
from typing import Any

from common.runner import create_trace_attributes
from telemetry_setup import setup_telemetry

from config import SAMPLES


def run_sample(name: str, args: Any) -> bool:
    """Run one deterministic Pydantic AI sample."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    print(f"Running sample: {name}")
    print(f"  Model: {args.model}")
    print(f"  SideSeat telemetry: {args.sideseat}")
    print()

    client = setup_telemetry(use_sideseat=args.sideseat)
    trace_attrs = create_trace_attributes("pydantic-ai", name)
    module = importlib.import_module(SAMPLES[name])
    try:
        module.run(trace_attrs, client)
    finally:
        client.shutdown()
    return True
