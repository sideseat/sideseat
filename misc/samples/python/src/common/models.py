"""Shared model configuration across all framework samples.

This module defines the canonical model aliases and their mappings to
provider-specific model IDs. Each framework translates these to their
own format in their respective runner.py files.
"""

from typing import NamedTuple


class ModelInfo(NamedTuple):
    """Model configuration info."""

    provider: str  # Provider name (bedrock, anthropic, openai, gemini)
    model_id: str  # Base model ID (without provider prefix)
    litellm_id: str  # LiteLLM format (provider/model_id)
    supports_thinking: bool  # Whether model supports extended thinking


# Canonical model definitions
# Each model has a base ID that frameworks translate to their format:
# - Strands: uses model_id directly with provider-specific client
# - CrewAI/ADK: uses litellm_id format
# - AutoGen: uses model_id with provider-specific client
# - OpenAI Agents: uses model_id (OpenAI only)

MODEL_ALIASES: dict[str, ModelInfo] = {
    # Bedrock models (via AWS Bedrock)
    "bedrock-haiku": ModelInfo(
        provider="bedrock",
        model_id="global.anthropic.claude-haiku-4-5-20251001-v1:0",
        litellm_id="bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0",
        supports_thinking=True,
    ),
    "bedrock-sonnet": ModelInfo(
        provider="bedrock",
        model_id="global.anthropic.claude-sonnet-4-20250514-v1:0",
        litellm_id="bedrock/global.anthropic.claude-sonnet-4-20250514-v1:0",
        supports_thinking=True,
    ),
    "bedrock-nova": ModelInfo(
        provider="bedrock",
        model_id="us.amazon.nova-2-lite-v1:0",
        litellm_id="bedrock/us.amazon.nova-2-lite-v1:0",
        supports_thinking=False,
    ),
    # Anthropic direct API
    "anthropic-haiku": ModelInfo(
        provider="anthropic",
        model_id="claude-haiku-4-5-20251001",
        litellm_id="anthropic/claude-haiku-4-5-20251001",
        supports_thinking=True,
    ),
    "anthropic-sonnet": ModelInfo(
        provider="anthropic",
        model_id="claude-sonnet-4-20250514",
        litellm_id="anthropic/claude-sonnet-4-20250514",
        supports_thinking=True,
    ),
    # OpenAI models
    "openai-gpt5nano": ModelInfo(
        provider="openai",
        model_id="gpt-5-nano-2025-08-07",
        litellm_id="openai/gpt-5-nano-2025-08-07",
        supports_thinking=True,
    ),
    # Gemini models (Strands only for now)
    "gemini-flash": ModelInfo(
        provider="gemini",
        model_id="gemini-2.0-flash",
        litellm_id="gemini/gemini-2.0-flash",
        supports_thinking=False,
    ),
}

# Set of models that support extended thinking/reasoning
REASONING_MODELS: set[str] = {
    alias for alias, info in MODEL_ALIASES.items() if info.supports_thinking
}

# Default budget_tokens for extended thinking (minimum is 1024)
DEFAULT_THINKING_BUDGET = 4096

# Standard sample names (shared across all frameworks)
SAMPLE_NAMES = [
    "tool_use",
    "mcp_tools",
    "structured_output",
    "files",
    "image_gen",
    "agent_core",
    "swarm",
    "rag_local",
    "reasoning",
    "error",
]


def get_model_info(alias: str) -> ModelInfo | None:
    """Get model info by alias.

    Args:
        alias: Model alias (e.g., 'bedrock-haiku')

    Returns:
        ModelInfo if found, None otherwise
    """
    return MODEL_ALIASES.get(alias)


def get_supported_models(provider: str | None = None) -> dict[str, ModelInfo]:
    """Get models filtered by provider.

    Args:
        provider: Optional provider filter (bedrock, anthropic, openai, gemini)

    Returns:
        Dict of alias -> ModelInfo for matching models
    """
    if provider is None:
        return MODEL_ALIASES.copy()
    return {alias: info for alias, info in MODEL_ALIASES.items() if info.provider == provider}
