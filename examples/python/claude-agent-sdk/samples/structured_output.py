"""Structured output via a JSON schema.

Demonstrates:
- output_format with a json_schema
- Reading ResultMessage.structured_output

The schema mirrors the Person model used by the other sample suites so the
extracted shape is comparable across frameworks in the SideSeat UI.
"""

import json

from agent_helpers import build_options, print_result, trace_scope
from claude_agent_sdk import ResultMessage, query

PERSON_SCHEMA = {
    "type": "object",
    "properties": {
        "name": {"type": "string", "description": "Full name of the person"},
        "age": {"type": "integer", "description": "Age in years"},
        "address": {
            "type": "object",
            "properties": {
                "street": {"type": "string"},
                "city": {"type": "string"},
                "country": {"type": "string"},
                "postal_code": {"type": "string"},
            },
            "required": ["street", "city", "country"],
            "additionalProperties": False,
        },
        "contacts": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "email": {"type": "string"},
                    "phone": {"type": "string"},
                },
                "additionalProperties": False,
            },
        },
        "skills": {"type": "array", "items": {"type": "string"}},
    },
    "required": ["name", "age", "address"],
    "additionalProperties": False,
}

PROMPT = (
    "Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, "
    "New York, USA. Email: jane@example.com"
)


async def run(model, trace_attrs: dict, client, env: dict):
    options = build_options(
        model,
        env,
        output_format={"type": "json_schema", "schema": PERSON_SCHEMA},
        system_prompt=(
            "You are an information extraction assistant. "
            "Extract the person information from the provided text."
        ),
    )

    with trace_scope(client, "claude-agent-structured-output", trace_attrs):
        print(f"--- {PROMPT} ---")
        async for message in query(prompt=PROMPT, options=options):
            if isinstance(message, ResultMessage):
                print(json.dumps(message.structured_output, indent=2))
                print_result(message)
