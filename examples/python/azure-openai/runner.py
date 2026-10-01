"""Azure OpenAI client setup and sample execution."""

import importlib
import os
import socket
from contextlib import contextmanager
from typing import Any, Iterator
from urllib.parse import urlsplit

from common.runner import create_trace_attributes, run_all_samples_base
from openai import OpenAI

from config import SAMPLES
from telemetry_setup import setup_telemetry

FIXTURE_HOST = "sideseat-fixture.openai.azure.com"
FIXTURE_BASE_URL = f"http://{FIXTURE_HOST}:5401/openai/v1/"


@contextmanager
def _fixture_dns(base_url: str) -> Iterator[None]:
    """Resolve only the documented local Azure-shaped host to the fixture server."""
    host = urlsplit(base_url).hostname
    if host != FIXTURE_HOST:
        yield
        return

    original = socket.getaddrinfo

    def resolve(name: str, *args: Any, **kwargs: Any) -> Any:
        return original(
            "127.0.0.1" if name == FIXTURE_HOST else name,
            *args,
            **kwargs,
        )

    socket.getaddrinfo = resolve
    try:
        for key in ("NO_PROXY", "no_proxy"):
            entries = [item for item in os.getenv(key, "").split(",") if item]
            if FIXTURE_HOST not in entries:
                os.environ[key] = ",".join([*entries, FIXTURE_HOST])
        yield
    finally:
        socket.getaddrinfo = original


def get_client(base_url: str) -> OpenAI:
    """Create the current OpenAI client against an Azure OpenAI v1 endpoint."""
    return OpenAI(
        api_key=os.getenv("AZURE_OPENAI_API_KEY", "fixture-key"),
        base_url=base_url,
        max_retries=0,
        timeout=30,
    )


def run_sample(name: str, args: Any) -> bool:
    """Run one Azure OpenAI sample and flush its telemetry."""
    if name not in SAMPLES:
        print(f"Unknown sample: {name}")
        return False

    owner = setup_telemetry(use_sideseat=args.sideseat)
    try:
        trace_attrs = create_trace_attributes("azure-openai", name)
        module = importlib.import_module(SAMPLES[name])
        base_url = os.getenv("AZURE_OPENAI_BASE_URL", FIXTURE_BASE_URL)
        with _fixture_dns(base_url):
            module.run(get_client(base_url), args.model, trace_attrs, owner)
        return True
    finally:
        owner.shutdown()


def run_all_samples(args: Any) -> None:
    """Run every Azure OpenAI sample."""
    run_all_samples_base(SAMPLES, run_sample, args)
