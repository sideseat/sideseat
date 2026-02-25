"""Configuration for Bedrock samples."""

from common.models import MODEL_ALIASES as _ALL_MODELS

# Bedrock samples only use the bedrock provider
SUPPORTED_PROVIDERS = {"bedrock"}

MODEL_ALIASES = {
    alias: (info.provider, info.model_id)
    for alias, info in _ALL_MODELS.items()
    if info.provider in SUPPORTED_PROVIDERS
}

DEFAULT_MODEL = "bedrock-haiku"

SAMPLE_NAMES = [
    "converse",
    "multi_turn",
    "invoke_model",
    "document",
    "session",
    "error",
]

SAMPLES = {name: f"bedrock_sample.samples.{name}" for name in SAMPLE_NAMES}
