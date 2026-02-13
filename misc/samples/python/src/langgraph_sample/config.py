"""Configuration for LangGraph samples."""

from common.models import (
    MODEL_ALIASES as _ALL_MODELS,
)
from common.models import (
    REASONING_MODELS as _ALL_REASONING,
)
from common.models import (
    SAMPLE_NAMES,
)

# LangGraph supports all providers via LangChain integrations
SUPPORTED_PROVIDERS = {"bedrock", "anthropic", "openai", "gemini"}

# Map to (provider, model_id) format for LangGraph
MODEL_ALIASES = {
    alias: (info.provider, info.model_id)
    for alias, info in _ALL_MODELS.items()
    if info.provider in SUPPORTED_PROVIDERS
}

# Reasoning models that LangGraph supports
REASONING_MODELS = {alias for alias in _ALL_REASONING if alias in MODEL_ALIASES}

# Default model alias
DEFAULT_MODEL = "bedrock-haiku"

# Sample module paths (excluding agent_core which LangGraph doesn't have yet)
SAMPLES = {
    name: f"langgraph_sample.samples.{name}"
    for name in SAMPLE_NAMES
    if name != "agent_core"  # LangGraph doesn't have agent_core sample
}
