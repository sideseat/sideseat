"""Configuration for Microsoft Agent Framework samples."""

from common.models import (
    MODEL_ALIASES as _ALL_MODELS,
)
from common.models import (
    REASONING_MODELS as _ALL_REASONING,
)
from common.models import (
    SAMPLE_NAMES,
)

# Agent Framework supports OpenAI and Anthropic directly
SUPPORTED_PROVIDERS = {"openai", "anthropic"}

# Filter to supported models only
MODEL_ALIASES = {
    alias: (info.provider, info.model_id)
    for alias, info in _ALL_MODELS.items()
    if info.provider in SUPPORTED_PROVIDERS
}

# Reasoning models that Agent Framework supports
REASONING_MODELS = {alias for alias in _ALL_REASONING if alias in MODEL_ALIASES}

# Default model alias
DEFAULT_MODEL = "openai-gpt5nano"

# Sample module paths
SAMPLES = {name: f"agent_framework_sample.samples.{name}" for name in SAMPLE_NAMES}
