"""AgentScope model setup and sample execution."""

import asyncio
import importlib
import os
from functools import partial
from typing import Any

from common.runner import create_trace_attributes, run_all_samples_base
from telemetry_setup import setup_telemetry

from config import SAMPLES


def get_model(model_id: str, *, streaming: bool = False) -> Any:
    """Create the current AgentScope OpenAI chat model."""
    from agentscope.credential import OpenAICredential
    from agentscope.model import OpenAIChatModel

    credential = OpenAICredential(
        api_key=os.getenv("OPENAI_API_KEY", "fixture-key"),
        base_url=os.getenv("OPENAI_BASE_URL"),
    )
    return OpenAIChatModel(
        credential=credential,
        model=model_id,
        stream=streaming,
        max_retries=0,
    )


def run_sample(name: str, args: Any) -> bool:
    """Run one AgentScope sample and flush its telemetry."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    owner = setup_telemetry(use_sideseat=args.sideseat)
    try:
        trace_attrs = create_trace_attributes("agentscope", name)
        module = importlib.import_module(SAMPLES[name])
        model_factory = partial(get_model, args.model)
        asyncio.run(module.run(model_factory, trace_attrs, owner, args.sideseat))
        return True
    finally:
        shutdown = getattr(owner, "shutdown", None)
        if shutdown is not None:
            shutdown()


def run_all_samples(args: Any) -> None:
    """Run every AgentScope sample."""
    run_all_samples_base(SAMPLES, run_sample, args)
