"""Strands Agents: emits GenAI telemetry through the global tracer provider."""

from __future__ import annotations

from typing import Any

from sideseat.integrations._base import Integration, SetupContext


class Strands(Integration):
    name = "strands"
    packages = ("strands-agents",)
    extra = None

    def prepare(self, ctx: SetupContext) -> None:
        # Strands serializes content blocks with a JSON encoder that replaces bytes with a
        # placeholder, which drops images and documents from telemetry. Encode them as base64
        # instead; the server moves them to file storage on ingest.
        from strands.telemetry import tracer

        from sideseat._encoding import encode_value

        def _process_value(self: Any, value: Any) -> Any:
            return encode_value(value)

        tracer.JSONEncoder._process_value = _process_value
