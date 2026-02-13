"""Configuration for AutoGen samples."""

from common.models import (
    MODEL_ALIASES as _ALL_MODELS,
)
from common.models import (
    REASONING_MODELS as _ALL_REASONING,
)
from common.models import (
    SAMPLE_NAMES,
)

# AutoGen supports Anthropic and OpenAI (not Bedrock natively)
SUPPORTED_PROVIDERS = {"anthropic", "openai"}

# Filter to supported models only
MODEL_ALIASES = {
    alias: (info.provider, info.model_id)
    for alias, info in _ALL_MODELS.items()
    if info.provider in SUPPORTED_PROVIDERS
}

# Reasoning models that AutoGen supports
REASONING_MODELS = {alias for alias in _ALL_REASONING if alias in MODEL_ALIASES}

# Default model alias (anthropic-haiku as closest to Strands' bedrock-haiku default)
DEFAULT_MODEL = "anthropic-haiku"

# Sample module paths
SAMPLES = {name: f"autogen_sample.samples.{name}" for name in SAMPLE_NAMES}
