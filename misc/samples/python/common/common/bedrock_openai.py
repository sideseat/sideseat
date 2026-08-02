"""OpenAI- and Anthropic-shaped clients pointed at Bedrock's compatible endpoints.

Bedrock exposes an OpenAI-shaped API at `/openai/v1` and an Anthropic-shaped one at
`/anthropic/v1` on the bedrock-runtime host, which lets the provider suites run on Bedrock
credentials instead of needing OPENAI_API_KEY or ANTHROPIC_API_KEY.

Both SDKs only know bearer auth, so requests are signed with SigV4 through a custom HTTP
client. That avoids minting a Bedrock API key, which would need an IAM user - the usual
setup here is an assumed role.

Model ids differ between the two surfaces. The OpenAI one takes plain Bedrock ids
(`openai.gpt-oss-20b-1:0`). The Anthropic one takes *inference profile* ids and only the
newer models: `us.` or `global.` prefixed `claude-sonnet-5`, `claude-opus-5`,
`claude-opus-4-8`, `claude-opus-4-7`. Versioned ids such as
`anthropic.claude-haiku-4-5-20251001-v1:0` are rejected with 404, and there is no haiku on
that surface.
"""

from __future__ import annotations

import os

import boto3
import httpx2
from botocore.auth import SigV4Auth
from botocore.awsrequest import AWSRequest
from typing import TYPE_CHECKING

if TYPE_CHECKING:  # pragma: no cover - typing only
    from anthropic import Anthropic, AsyncAnthropic
    from openai import AsyncOpenAI, OpenAI

# Models served by the OpenAI-compatible endpoint. Anthropic models are not reachable
# through it - use the native Bedrock path for those.
DEFAULT_BEDROCK_OPENAI_MODEL = "openai.gpt-oss-20b-1:0"

# Cheapest model the Anthropic-compatible surface accepts here.
DEFAULT_BEDROCK_ANTHROPIC_MODEL = "us.anthropic.claude-sonnet-5"


def bedrock_openai_base_url(region: str | None = None) -> str:
    return f"https://bedrock-runtime.{region or _region()}.amazonaws.com/openai/v1"


def bedrock_anthropic_base_url(region: str | None = None) -> str:
    """Base URL for the Anthropic-compatible endpoint.

    Deliberately without the `/v1`: the Anthropic SDK appends `/v1/messages` itself, and
    including it here yields `/anthropic/v1/v1/messages`. That path is not routed to the
    Anthropic surface, so Bedrock answers with a Coral envelope (`{"Output":..,
    "Version":..}`) that the SDK parses into a Message whose fields are all None - which
    then shows up as `messages.create()` returning None once logfire wraps it.
    """
    return f"https://bedrock-runtime.{region or _region()}.amazonaws.com/anthropic"


class _SigV4Auth(httpx2.Auth):
    """Signs each outgoing request with SigV4.

    Credentials are fetched per request rather than cached so that expiring session
    credentials keep working across a long sample run.
    """

    requires_request_body = True

    def __init__(self, region: str, service: str = "bedrock") -> None:
        self._region = region
        self._service = service
        self._session = boto3.Session()

    def auth_flow(self, request):  # type: ignore[no-untyped-def]
        credentials = self._session.get_credentials()
        if credentials is None:
            raise RuntimeError(
                "No AWS credentials found for the Bedrock OpenAI-compatible endpoint"
            )
        frozen = credentials.get_frozen_credentials()
        aws_request = AWSRequest(
            method=request.method,
            url=str(request.url),
            data=request.content or b"",
            headers={
                "Content-Type": request.headers.get("Content-Type", "application/json")
            },
        )
        SigV4Auth(frozen, self._service, self._region).add_auth(aws_request)
        for name, value in aws_request.headers.items():
            request.headers[name] = value
        # The SDKs put the (unused) api_key in x-api-key. Bedrock then tries API-key auth
        # instead of the SigV4 Authorization header and answers 400, so drop it.
        if "x-api-key" in request.headers:
            del request.headers["x-api-key"]
        yield request


def _region() -> str:
    return os.getenv("AWS_DEFAULT_REGION") or os.getenv("AWS_REGION") or "us-west-2"


def bedrock_openai_client() -> "OpenAI":
    """Synchronous OpenAI client talking to Bedrock."""
    from openai import OpenAI

    region = _region()
    return OpenAI(
        base_url=bedrock_openai_base_url(region),
        # Required by the SDK but unused: _SigV4Auth supplies the real Authorization.
        api_key="sigv4",
        http_client=httpx2.Client(auth=_SigV4Auth(region), timeout=120.0),
    )


def bedrock_async_openai_client() -> "AsyncOpenAI":
    """Asynchronous variant, for suites built on AsyncOpenAI."""
    from openai import AsyncOpenAI

    region = _region()
    return AsyncOpenAI(
        base_url=bedrock_openai_base_url(region),
        api_key="sigv4",
        http_client=httpx2.AsyncClient(auth=_SigV4Auth(region), timeout=120.0),
    )


def bedrock_anthropic_client() -> "Anthropic":
    """Synchronous Anthropic client talking to Bedrock's Anthropic-compatible endpoint."""
    from anthropic import Anthropic

    region = _region()
    return Anthropic(
        base_url=bedrock_anthropic_base_url(region),
        api_key="sigv4",
        http_client=httpx2.Client(auth=_SigV4Auth(region), timeout=120.0),
    )


def bedrock_async_anthropic_client() -> "AsyncAnthropic":
    """Asynchronous variant."""
    from anthropic import AsyncAnthropic

    region = _region()
    return AsyncAnthropic(
        base_url=bedrock_anthropic_base_url(region),
        api_key="sigv4",
        http_client=httpx2.AsyncClient(auth=_SigV4Auth(region), timeout=120.0),
    )
