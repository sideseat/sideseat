"""Error telemetry.

Queries with a nonexistent model ID so the CLI's API error surfaces on the span.
Mirrors the error sample in every other suite, so `all` intentionally reports a
failure here.
"""

from agent_helpers import build_options, print_stream, trace_scope
from claude_agent_sdk import query

INVALID_MODEL_ID = "nonexistent-model-id-12345"


class _InvalidModel:
    model_id = INVALID_MODEL_ID


async def run(model, trace_attrs: dict, client, env: dict):
    # Override both the primary and background model pins that telemetry_setup
    # applied, otherwise the valid pin would mask the failure.
    error_env = {
        **env,
        "ANTHROPIC_MODEL": INVALID_MODEL_ID,
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": INVALID_MODEL_ID,
    }

    options = build_options(_InvalidModel(), error_env, allowed_tools=[], max_turns=1)

    with trace_scope(client, "claude-agent-error", trace_attrs):
        await print_stream(query(prompt="What is 2 + 2?", options=options))
