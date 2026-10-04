"""Maps a harness model alias to the Claude Code CLI's provider configuration."""

from __future__ import annotations

import os
from dataclasses import dataclass, field

from harness import Model
from harness.clients import fake_url
from harness.models import region


@dataclass(frozen=True)
class ClaudeModel:
    id: str
    #: Environment for the CLI: the provider, and this model in every model slot.
    env: dict[str, str] = field(default_factory=dict)


def build(model: Model) -> ClaudeModel:
    # Background requests (titles, summaries) use the small-model slot; pinning it to the same model
    # keeps every request on a model the account has enabled.
    slots = {
        "ANTHROPIC_MODEL": model.id,
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": model.id,
        "ANTHROPIC_SMALL_FAST_MODEL": model.id,
    }
    if model.surface == "fake-anthropic":
        # The local fake Messages endpoint, for credential-free smoke runs.
        env = {
            "CLAUDE_CODE_USE_BEDROCK": "0",
            "ANTHROPIC_BASE_URL": fake_url(model.surface),
            "ANTHROPIC_API_KEY": "fake",
        }
        return ClaudeModel(id=model.id, env={**env, **slots})
    if (
        model.surface not in {"bedrock", "bedrock-anthropic"}
        or "anthropic." not in model.id
    ):
        raise SystemExit(
            f"the Claude Agent SDK runs Claude models on Bedrock; {model.alias} is {model.id}"
        )
    env = {"CLAUDE_CODE_USE_BEDROCK": "1", "AWS_REGION": region()}
    if proxy := os.getenv("SIDESEAT_MODEL_PROXY"):
        # The capture proxy signs for Bedrock itself, so the CLI sends it unsigned requests.
        env["ANTHROPIC_BEDROCK_BASE_URL"] = proxy
        env["CLAUDE_CODE_SKIP_BEDROCK_AUTH"] = "1"
    return ClaudeModel(id=model.id, env={**env, **slots})
