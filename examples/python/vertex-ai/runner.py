"""Google Vertex AI client setup and sample execution."""

import importlib
import os
from typing import Any, NamedTuple

from common.runner import create_trace_attributes, run_all_samples_base
from google import genai
from google.genai import types
from google.oauth2.credentials import Credentials

from config import SAMPLES
from telemetry_setup import setup_telemetry

FIXTURE_BASE_URL = "http://127.0.0.1:5404"
FIXTURE_PROJECT = "sideseat-fixture"
DEFAULT_LOCATION = "us-central1"


class VertexModel(NamedTuple):
    """Google Gen AI client configured for Vertex AI and paired with a model ID."""

    client: genai.Client
    model_id: str


def get_model(model_id: str) -> VertexModel:
    """Create the current unified Google client in Vertex AI mode."""
    base_url = os.getenv("VERTEX_AI_BASE_URL")
    project = os.getenv("GOOGLE_CLOUD_PROJECT") or (
        FIXTURE_PROJECT if base_url else None
    )
    location = os.getenv("GOOGLE_CLOUD_LOCATION", DEFAULT_LOCATION)
    kwargs: dict[str, Any] = {
        "vertexai": True,
        "project": project,
        "location": location,
    }
    if base_url:
        kwargs["credentials"] = Credentials(token="sideseat-fixture-token")
        kwargs["http_options"] = types.HttpOptions(base_url=base_url)

    return VertexModel(client=genai.Client(**kwargs), model_id=model_id)


def run_sample(name: str, args: Any) -> bool:
    """Run one Vertex AI sample and flush its telemetry."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    owner = setup_telemetry(use_sideseat=args.sideseat)
    model = get_model(args.model)
    try:
        trace_attrs = create_trace_attributes("vertex-ai", name)
        module = importlib.import_module(SAMPLES[name])
        module.run(model, trace_attrs, owner)
        return True
    finally:
        model.client.close()
        owner.shutdown()


def run_all_samples(args: Any) -> None:
    """Run every Google Vertex AI sample."""
    run_all_samples_base(SAMPLES, run_sample, args)
