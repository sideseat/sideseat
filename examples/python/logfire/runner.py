"""Generic Logfire sample execution."""

import importlib
from typing import Any

from common.runner import create_trace_attributes, run_all_samples_base
from telemetry_setup import setup_telemetry

from config import SAMPLES


def run_sample(name: str, args: Any) -> bool:
    """Run one deterministic Logfire sample and flush its telemetry."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    owner = setup_telemetry(use_sideseat=args.sideseat)
    try:
        trace_attrs = create_trace_attributes("logfire", name)
        module = importlib.import_module(SAMPLES[name])
        module.run(args.model, trace_attrs, owner)
        return True
    finally:
        owner.shutdown()


def run_all_samples(args: Any) -> None:
    """Run every generic Logfire sample."""
    run_all_samples_base(SAMPLES, run_sample, args)
