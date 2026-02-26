"""Shared constants and helpers for AWS instrumentation."""

from __future__ import annotations

from typing import TYPE_CHECKING

from opentelemetry import trace

if TYPE_CHECKING:
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.trace import Tracer

# Gen AI semantic convention attribute keys
SYSTEM = "gen_ai.system"
PROVIDER_NAME = "gen_ai.provider.name"
OPERATION = "gen_ai.operation.name"
REQUEST_MODEL = "gen_ai.request.model"
RESPONSE_MODEL = "gen_ai.response.model"
INPUT_TOKENS = "gen_ai.usage.input_tokens"
OUTPUT_TOKENS = "gen_ai.usage.output_tokens"
CACHE_READ_TOKENS = "gen_ai.usage.cache_read_input_tokens"
CACHE_WRITE_TOKENS = "gen_ai.usage.cache_write_input_tokens"
FINISH_REASONS = "gen_ai.response.finish_reasons"
TOOL_DEFINITIONS = "gen_ai.tool.definitions"
TEMPERATURE = "gen_ai.request.temperature"
TOP_P = "gen_ai.request.top_p"
MAX_TOKENS = "gen_ai.request.max_tokens"
AGENT_ID = "gen_ai.agent.id"

SYSTEM_VALUE = "aws_bedrock"


def get_tracer(provider: TracerProvider | None, name: str) -> Tracer:
    """Get tracer from explicit provider or global."""
    if provider is not None:
        return provider.get_tracer(name)
    return trace.get_tracer(name)
