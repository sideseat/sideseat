"""Error handling sample — calls Bedrock with an invalid model ID.

Demonstrates:
- Error telemetry capture (exception type, message, stacktrace)
- Error span status in traces
"""

from sideseat import SideSeat


def run(bedrock, trace_attrs: dict, client: SideSeat):
    """Run converse with an invalid model ID to generate error telemetry."""
    with client.trace("bedrock-error"):
        bedrock.client.converse(
            modelId="nonexistent-model-id-12345",
            messages=[{"role": "user", "content": [{"text": "Hello"}]}],
        )
