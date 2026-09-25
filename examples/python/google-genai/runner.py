"""Google GenAI sample runner."""

import importlib
import os
from typing import Any, NamedTuple

from common.runner import create_trace_attributes
from google import genai
from google.genai import types
from telemetry_setup import setup_telemetry

from config import SAMPLES


class GoogleModel(NamedTuple):
    """Google GenAI client paired with a model ID."""

    client: genai.Client
    model_id: str


def get_model(model_id: str) -> GoogleModel:
    """Create a real Google GenAI client, optionally pointed at the fake endpoint."""
    base_url = os.getenv("GOOGLE_GENAI_BASE_URL")
    http_options = types.HttpOptions(base_url=base_url) if base_url else None
    api_key = (
        "sideseat-fixture-key"
        if base_url
        else os.getenv("GEMINI_API_KEY") or os.getenv("GOOGLE_API_KEY")
    )
    client = genai.Client(api_key=api_key, http_options=http_options)
    return GoogleModel(client=client, model_id=model_id)


def run_sample(name: str, args: Any) -> bool:
    """Run one Google GenAI sample."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    print(f"Running sample: {name}")
    print(f"  Model: {args.model}")
    print(f"  SideSeat telemetry: {args.sideseat}")
    print()

    trace_attrs = create_trace_attributes("google-genai", name)
    trace_client = setup_telemetry(use_sideseat=args.sideseat)
    model = get_model(args.model)
    module = importlib.import_module(SAMPLES[name])
    try:
        module.run(model, trace_attrs, trace_client)
    finally:
        model.client.close()
        trace_client.shutdown()
    return True
