"""Configuration for OpenAI Agents samples."""

from common.models import (
    MODEL_ALIASES as _ALL_MODELS,
)
from common.models import (
    REASONING_MODELS as _ALL_REASONING,
)
from common.models import (
    SAMPLE_NAMES,
)

# OpenAI Agents SDK only supports OpenAI models
SUPPORTED_PROVIDERS = {"openai"}

# Filter to OpenAI models only, use model_id directly
MODEL_ALIASES = {
    alias: info.model_id
    for alias, info in _ALL_MODELS.items()
    if info.provider in SUPPORTED_PROVIDERS
}

# Reasoning models that OpenAI Agents supports
REASONING_MODELS = {alias for alias in _ALL_REASONING if alias in MODEL_ALIASES}

# Default model alias
DEFAULT_MODEL = "openai-gpt5nano"

# Sample module paths
SAMPLES = {name: f"openai_agents_sample.samples.{name}" for name in SAMPLE_NAMES}
