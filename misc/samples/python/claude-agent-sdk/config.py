"""Configuration for Claude Agent SDK samples."""

from common.models import MODEL_ALIASES as _ALL_MODELS

# Claude Code runs against Bedrock via CLAUDE_CODE_USE_BEDROCK=1.
SUPPORTED_PROVIDERS = {"bedrock"}

MODEL_ALIASES = {
    alias: (info.provider, info.model_id)
    for alias, info in _ALL_MODELS.items()
    if info.provider in SUPPORTED_PROVIDERS
}

DEFAULT_MODEL = "bedrock-haiku"

# Shared sample names that map onto an agentic harness.
SHARED_SAMPLES = ["tool_use", "mcp_tools", "structured_output", "reasoning", "error"]

# Samples specific to the Claude Agent SDK.
CLAUDE_ONLY_SAMPLES = ["custom_tools", "subagents", "multi_turn", "permissions"]

SAMPLE_NAMES = SHARED_SAMPLES + CLAUDE_ONLY_SAMPLES
SAMPLES = {name: f"samples.{name}" for name in SAMPLE_NAMES}
