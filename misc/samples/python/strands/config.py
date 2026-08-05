"""Configuration for Strands samples."""

from common.models import (
    MODEL_ALIASES as _ALL_MODELS,
)
from common.models import (
    REASONING_MODELS as _ALL_REASONING,
)
from common.models import (
    SAMPLE_NAMES,
)

# Strands supports all providers natively
SUPPORTED_PROVIDERS = {"bedrock", "anthropic", "openai", "gemini"}

# Map to (provider, model_id) format for Strands
MODEL_ALIASES = {
    alias: (info.provider, info.model_id)
    for alias, info in _ALL_MODELS.items()
    if info.provider in SUPPORTED_PROVIDERS
}

# Reasoning models that Strands supports
REASONING_MODELS = {alias for alias in _ALL_REASONING if alias in MODEL_ALIASES}

# Default model alias
DEFAULT_MODEL = "bedrock-haiku"

# Strands-only samples (not part of the cross-provider SAMPLE_NAMES set).
# Demonstrate the SideSeat WS presence/introspection bridge.
STRANDS_ONLY_SAMPLES = ["strands_ws"]

# Sample module paths
SAMPLES = {name: f"samples.{name}" for name in SAMPLE_NAMES}
SAMPLES.update({name: f"samples.{name}" for name in STRANDS_ONLY_SAMPLES})

# What `strands all` runs. strands_ws is excluded: it calls connect(block=True) and
# blocks until SIGINT, so including it made `all` hang forever instead of finishing.
BATCH_SAMPLES = {name: path for name, path in SAMPLES.items() if name in SAMPLE_NAMES}
