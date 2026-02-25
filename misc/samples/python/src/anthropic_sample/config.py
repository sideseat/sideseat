"""Configuration for Anthropic samples."""

from common.models import MODEL_ALIASES as _ALL_MODELS

# Anthropic samples only use the anthropic provider
SUPPORTED_PROVIDERS = {"anthropic"}

MODEL_ALIASES = {
    alias: (info.provider, info.model_id)
    for alias, info in _ALL_MODELS.items()
    if info.provider in SUPPORTED_PROVIDERS
}

DEFAULT_MODEL = "anthropic-haiku"

SAMPLE_NAMES = [
    "messages",
    "multi_turn",
    "thinking",
    "vision",
    "document",
    "session",
    "error",
]

SAMPLES = {name: f"anthropic_sample.samples.{name}" for name in SAMPLE_NAMES}
