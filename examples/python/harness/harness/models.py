"""Model aliases every suite accepts with ``--model``.

All live models run on Amazon Bedrock, so one set of AWS credentials runs every suite. The
``fake-*`` aliases point at the local fake servers in :mod:`harness.fakes` and need no credentials.
"""

from __future__ import annotations

import os
from dataclasses import dataclass


@dataclass(frozen=True)
class Model:
    alias: str
    #: Which API surface serves the model: ``bedrock`` (Converse), ``bedrock-openai`` (Bedrock's
    #: OpenAI-compatible endpoint), ``bedrock-anthropic`` (the Anthropic SDK on bedrock-runtime), or ``fake-*``.
    surface: str
    id: str
    reasoning: bool

    @property
    def litellm_id(self) -> str:
        prefix = {
            "bedrock": "bedrock",
            "bedrock-anthropic": "bedrock",
            "bedrock-openai": "bedrock",
        }
        return f"{prefix.get(self.surface, 'openai')}/{self.id}"


MODELS: dict[str, Model] = {
    m.alias: m
    for m in (
        Model(
            "sonnet", "bedrock", "global.anthropic.claude-sonnet-5-5", reasoning=True
        ),
        Model(
            "haiku",
            "bedrock",
            "global.anthropic.claude-haiku-4-5-20251001-v1:0",
            reasoning=True,
        ),
        Model("opus", "bedrock", "global.anthropic.claude-opus-5-5", reasoning=True),
        Model("nova", "bedrock", "global.amazon.nova-2-lite-v1:0", reasoning=False),
        # The Anthropic SDK through bedrock-runtime (AnthropicBedrock), for suites built on that SDK.
        Model(
            "claude",
            "bedrock-anthropic",
            "global.anthropic.claude-sonnet-5-5",
            reasoning=True,
        ),
        Model("gpt", "bedrock-openai", "global.openai.gpt-6.1-sol", reasoning=True),
        Model("fake-openai", "fake-openai", "gpt-6.1-sol", reasoning=False),
        Model("fake-anthropic", "fake-anthropic", "claude-sonnet-5-5", reasoning=False),
        Model("fake-gemini", "fake-gemini", "gemini-flash-latest", reasoning=False),
    )
}

DEFAULT = "sonnet"


def resolve(alias: str) -> Model:
    try:
        return MODELS[alias]
    except KeyError:
        known = ", ".join(MODELS)
        raise SystemExit(f"unknown model {alias!r}; choose one of: {known}") from None


def region() -> str:
    return os.getenv("AWS_REGION") or os.getenv("AWS_DEFAULT_REGION") or "us-east-1"
