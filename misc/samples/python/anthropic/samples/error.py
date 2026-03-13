"""Error handling sample — calls Anthropic with an invalid model ID.

Demonstrates:
- Error telemetry capture (exception type, message, stacktrace)
- Error span status in traces
"""

from sideseat import SideSeat


def run(model, trace_attrs: dict, client: SideSeat):
    """Run Messages API with an invalid model ID to generate error telemetry."""
    with client.trace("anthropic-error"):
        model.client.messages.create(
            model="nonexistent-model-id-12345",
            messages=[{"role": "user", "content": "Hello"}],
            max_tokens=1024,
        )
