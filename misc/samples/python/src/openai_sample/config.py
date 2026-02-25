"""Configuration for OpenAI samples."""

from common.models import MODEL_ALIASES as _ALL_MODELS

# OpenAI samples only use the openai provider
SUPPORTED_PROVIDERS = {"openai"}

MODEL_ALIASES = {
    alias: (info.provider, info.model_id)
    for alias, info in _ALL_MODELS.items()
    if info.provider in SUPPORTED_PROVIDERS
}

DEFAULT_MODEL = "openai-gpt5nano"

SAMPLE_NAMES = [
    "chat_completions",
    "responses",
    "multi_turn",
    "vision",
    "session",
    "error",
]

SAMPLES = {name: f"openai_sample.samples.{name}" for name in SAMPLE_NAMES}
