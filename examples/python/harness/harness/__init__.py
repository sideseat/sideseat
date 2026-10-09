"""Shared harness for the Python example suites: one CLI, one model catalog, two telemetry modes."""

import os

from harness import litellm_models
from harness.catalog import CATALOG
from harness.models import MODELS, Model
from harness.run import Run

# Before any suite imports LiteLLM, which would otherwise download its model map (`litellm_models`).
os.environ.update(litellm_models.ENV)

__all__ = ["CATALOG", "MODELS", "Model", "Run"]
