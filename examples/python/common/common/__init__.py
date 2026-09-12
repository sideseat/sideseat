"""Common utilities shared across all framework samples.

Provides:
- models: Canonical model definitions and aliases
- runner: Sample execution utilities
- telemetry: Base telemetry setup

Usage:
    from common.models import MODEL_ALIASES, REASONING_MODELS
    from common.runner import create_trace_attributes
    from common.telemetry import setup_base_telemetry
"""

from common.content import first_text_block
from common.images import generate_image_bedrock
from common.models import (
    DEFAULT_THINKING_BUDGET,
    MODEL_ALIASES,
    REASONING_MODELS,
    SAMPLE_NAMES,
    ModelInfo,
    get_model_info,
    get_supported_models,
)

__all__ = [
    "first_text_block",
    "generate_image_bedrock",
    # Models
    "MODEL_ALIASES",
    "REASONING_MODELS",
    "DEFAULT_THINKING_BUDGET",
    "SAMPLE_NAMES",
    "ModelInfo",
    "get_model_info",
    "get_supported_models",
]
