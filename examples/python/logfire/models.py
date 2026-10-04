"""Maps a harness model alias to a Logfire-instrumented OpenAI client and model id."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import logfire

from harness import Model
from harness.clients import openai_client


@dataclass(frozen=True)
class OpenAIModel:
    id: str
    client: Any


def build(model: Model) -> OpenAIModel:
    if model.surface not in {"bedrock-openai", "fake-openai"}:
        raise SystemExit(
            f"the Logfire suite runs the OpenAI SDK; {model.alias} is {model.surface}"
        )
    client = openai_client(model)
    # The application instruments its client once Logfire is configured, in either mode.
    logfire.instrument_openai(client)
    return OpenAIModel(model.id, client)
