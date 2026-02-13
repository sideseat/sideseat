"""Configuration for Google ADK samples."""

from common.models import (
    MODEL_ALIASES as _ALL_MODELS,
)
from common.models import (
    REASONING_MODELS as _ALL_REASONING,
)
from common.models import (
    SAMPLE_NAMES,
)

# Google ADK uses LiteLLM for non-Google models
SUPPORTED_PROVIDERS = {"bedrock", "anthropic", "openai"}

# Filter to supported models, use LiteLLM format
MODEL_ALIASES = {
    alias: (info.provider, info.litellm_id)
    for alias, info in _ALL_MODELS.items()
    if info.provider in SUPPORTED_PROVIDERS
}

# Reasoning models that ADK supports
REASONING_MODELS = {alias for alias in _ALL_REASONING if alias in MODEL_ALIASES}

# Default model alias
DEFAULT_MODEL = "bedrock-haiku"

# Sample module paths
SAMPLES = {name: f"adk_sample.samples.{name}" for name in SAMPLE_NAMES}
