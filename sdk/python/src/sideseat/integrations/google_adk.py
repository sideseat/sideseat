"""Google ADK: emits GenAI telemetry through the global provider; restores inline media."""

from __future__ import annotations

import base64
import functools
from typing import Any, cast

from sideseat.integrations._base import Integration, SetupContext


class GoogleADK(Integration):
    name = "google-adk"
    packages = ("google-adk",)

    def __init__(self) -> None:
        self._original: Any = None

    def prepare(self, ctx: SetupContext) -> None:
        if not ctx.capture_content:
            return
        from google.adk.telemetry import tracing

        original = tracing._build_llm_request_for_trace

        @functools.wraps(original)
        def with_inline_data(llm_request: Any) -> dict[str, Any]:
            return _weave_inline_data(llm_request, cast(dict[str, Any], original(llm_request)))

        tracing._build_llm_request_for_trace = with_inline_data
        self._original = original

    def shutdown(self) -> None:
        if self._original is not None:
            from google.adk.telemetry import tracing

            tracing._build_llm_request_for_trace = self._original
            self._original = None


def _weave_inline_data(llm_request: Any, sanitized: dict[str, Any]) -> dict[str, Any]:
    """Put back the inline images and documents ADK strips from its trace copy of a request.

    ADK's sanitizer stays the source of truth for every other field, including the credential-
    bearing HTTP options it excludes; only the omitted inline parts are restored, in their original
    positions.
    """
    contents = sanitized.get("contents")
    if not isinstance(contents, list):
        return sanitized
    for source, target in zip(llm_request.contents, contents, strict=False):
        parts = target.get("parts") if isinstance(target, dict) else None
        if not isinstance(parts, list):
            continue
        source_parts = list(source.parts or ())
        if len(parts) != sum(not part.inline_data for part in source_parts):
            continue  # ADK changed shape; leave its output untouched rather than misalign parts.
        remaining = iter(parts)
        target["parts"] = [
            {
                "inline_data": {
                    "mime_type": part.inline_data.mime_type,
                    "data": base64.b64encode(part.inline_data.data or b"").decode("ascii"),
                }
            }
            if part.inline_data
            else next(remaining)
            for part in source_parts
        ]
    return sanitized
