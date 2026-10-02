"""Provider clients for each model surface, so suites do not repeat endpoint and auth wiring.

Bedrock's OpenAI-compatible endpoint (``/openai/v1`` on bedrock-runtime) authenticates with SigV4,
which the OpenAI SDK cannot do on its own; :class:`SigV4` signs each request with the ambient AWS
credentials so no Bedrock API key has to be minted. Anthropic's SDK has a first-party bedrock-runtime client, ``AnthropicBedrock``.
"""

from __future__ import annotations

import os
from typing import Any

from harness.models import Model, region

FAKE_URLS = {
    "fake-openai": "http://127.0.0.1:5401/v1",
    "fake-anthropic": "http://127.0.0.1:5402",
    "fake-gemini": "http://127.0.0.1:5403",
}


def fake_url(surface: str) -> str:
    return os.getenv(f"{surface.upper().replace('-', '_')}_URL", FAKE_URLS[surface])


def bedrock_runtime_url() -> str:
    """bedrock-runtime, or the capture proxy standing in front of it."""
    return (
        os.getenv("SIDESEAT_MODEL_PROXY")
        or f"https://bedrock-runtime.{region()}.amazonaws.com"
    )


def bedrock_openai_base_url() -> str:
    return f"{bedrock_runtime_url()}/openai/v1"


def openai_client(model: Model, *, asynchronous: bool = False) -> Any:
    """An OpenAI SDK client for ``bedrock-openai`` or ``fake-openai`` models."""
    import httpx2
    from openai import AsyncOpenAI, OpenAI

    cls = AsyncOpenAI if asynchronous else OpenAI
    if model.surface == "fake-openai":
        return cls(base_url=fake_url(model.surface), api_key="fake")
    if model.surface != "bedrock-openai":
        raise SystemExit(
            f"model {model.alias} is not served by an OpenAI-compatible API"
        )
    http = (httpx2.AsyncClient if asynchronous else httpx2.Client)(
        auth=sigv4(), timeout=180.0
    )
    # The SDK requires an API key; SigV4 replaces the Authorization header it produces.
    return cls(base_url=bedrock_openai_base_url(), api_key="sigv4", http_client=http)


def anthropic_client(model: Model, *, asynchronous: bool = False) -> Any:
    """An Anthropic SDK client for ``bedrock-anthropic`` or ``fake-anthropic`` models."""
    import anthropic

    if model.surface == "fake-anthropic":
        cls = anthropic.AsyncAnthropic if asynchronous else anthropic.Anthropic
        return cls(base_url=fake_url(model.surface), api_key="fake")
    if model.surface != "bedrock-anthropic":
        raise SystemExit(f"model {model.alias} is not a Claude model")
    bedrock = (
        anthropic.AsyncAnthropicBedrock if asynchronous else anthropic.AnthropicBedrock
    )
    return bedrock(aws_region=region(), base_url=bedrock_runtime_url())


def sigv4() -> Any:
    """httpx2 auth that signs requests for Bedrock with the default AWS credential chain."""
    import boto3
    import httpx2
    from botocore.auth import SigV4Auth
    from botocore.awsrequest import AWSRequest

    session = boto3.Session()
    signing_region = region()

    class SigV4(httpx2.Auth):  # type: ignore[misc]
        requires_request_body = True

        def auth_flow(self, request: Any) -> Any:
            credentials = session.get_credentials()
            if credentials is None:
                raise RuntimeError("no AWS credentials for Bedrock")
            signed = AWSRequest(
                method=request.method,
                url=str(request.url),
                data=request.content or b"",
                headers={
                    "Content-Type": request.headers.get(
                        "Content-Type", "application/json"
                    )
                },
            )
            SigV4Auth(
                credentials.get_frozen_credentials(), "bedrock", signing_region
            ).add_auth(signed)
            for name, value in signed.headers.items():
                request.headers[name] = value
            # With an x-api-key header present Bedrock attempts key auth and rejects the request.
            request.headers.pop("x-api-key", None)
            yield request

    return SigV4()
