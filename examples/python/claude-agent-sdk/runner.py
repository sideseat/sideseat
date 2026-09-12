"""Sample runner for Claude Agent SDK."""

import asyncio
import importlib
from typing import NamedTuple

from common.runner import create_trace_attributes, run_all_samples_base
from config import MODEL_ALIASES, SAMPLES
from telemetry_setup import setup_telemetry


class ClaudeModel(NamedTuple):
    """Resolved Bedrock model ID for the Claude Code CLI."""

    model_id: str


def get_model(model_alias: str) -> ClaudeModel:
    if model_alias in MODEL_ALIASES:
        _, model_id = MODEL_ALIASES[model_alias]
    else:
        model_id = model_alias
    return ClaudeModel(model_id=model_id)


def run_sample(name: str, args) -> bool:
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    print(f"Running sample: {name}")
    print(f"  Model: {args.model}")
    print(f"  SideSeat telemetry: {args.sideseat}")
    print()

    trace_attrs = create_trace_attributes("claude-agent-sdk", name)
    model = get_model(args.model)
    client, env = setup_telemetry(use_sideseat=args.sideseat, model_id=model.model_id)

    module = importlib.import_module(SAMPLES[name])
    asyncio.run(module.run(model, trace_attrs, client, env))
    return True


def run_all_samples(args):
    run_all_samples_base(SAMPLES, run_sample, args)
