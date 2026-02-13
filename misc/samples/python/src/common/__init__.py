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
    # Models
    "MODEL_ALIASES",
    "REASONING_MODELS",
    "DEFAULT_THINKING_BUDGET",
    "SAMPLE_NAMES",
    "ModelInfo",
    "get_model_info",
    "get_supported_models",
]
