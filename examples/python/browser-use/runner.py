"""BrowserUse model setup and sample execution."""

import asyncio
import importlib
import os
from typing import Any

from common.runner import create_trace_attributes
from telemetry_setup import setup_telemetry

from config import SAMPLES


def get_model(model_id: str) -> Any:
    """Create the current BrowserUse OpenAI chat model."""
    from browser_use import ChatOpenAI

    return ChatOpenAI(
        model=model_id,
        api_key=os.getenv("OPENAI_API_KEY", "sideseat-local"),
        base_url=os.getenv("OPENAI_BASE_URL"),
        max_retries=0,
    )


def run_sample(name: str, args: Any) -> bool:
    """Run one BrowserUse sample and flush its telemetry."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    print(f"Running sample: {name}")
    print(f"  Model: {args.model}")
    print(f"  SideSeat telemetry: {args.sideseat}")
    print()

    os.environ.setdefault("BROWSER_USE_VERSION_CHECK", "false")
    os.environ.setdefault("BROWSER_USE_DISABLE_EXTENSIONS", "1")
    owner = setup_telemetry(use_sideseat=args.sideseat)
    try:
        trace_attrs = create_trace_attributes("browser-use", name)
        module = importlib.import_module(SAMPLES[name])
        asyncio.run(module.run(get_model(args.model), trace_attrs, owner))
        return True
    finally:
        owner.shutdown()
