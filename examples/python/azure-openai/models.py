"""Maps a harness model alias to an ``AzureOpenAI`` client and deployment name.

Bedrock credentials cannot reach Azure, so the client talks to the harness's fake OpenAI server. It
is still configured as an Azure application configures it - a resource endpoint, an API version, a
deployment - and only its transport is redirected: OpenInference identifies Azure by the client's
host, so the endpoint keeps an Azure resource name while requests go to the local server.
"""

from dataclasses import dataclass
from typing import Any
from urllib.parse import urlsplit

import httpx2
from openai import AzureOpenAI

from harness import Model
from harness.clients import fake_url

ENDPOINT = "https://sideseat-example.openai.azure.com"
#: The first API version that serves the Responses API on Azure.
API_VERSION = "2025-04-01-preview"


@dataclass(frozen=True)
class Deployment:
    client: AzureOpenAI
    #: The deployment name, which Azure takes in place of a model id.
    id: str


class _ToLocal(httpx2.HTTPTransport):  # type: ignore[misc]
    def __init__(self, url: str) -> None:
        super().__init__()
        self._target = urlsplit(url)

    def handle_request(self, request: Any) -> Any:
        request.url = request.url.copy_with(
            scheme=self._target.scheme,
            host=self._target.hostname,
            port=self._target.port,
        )
        return super().handle_request(request)


def build(model: Model) -> Deployment:
    if model.surface != "fake-openai":
        raise SystemExit(
            f"the Azure OpenAI suite runs the local OpenAI endpoint; {model.alias} is {model.surface}"
        )
    client = AzureOpenAI(
        azure_endpoint=ENDPOINT,
        api_key="fake",
        api_version=API_VERSION,
        http_client=httpx2.Client(transport=_ToLocal(fake_url(model.surface))),
    )
    return Deployment(client=client, id=model.id)
