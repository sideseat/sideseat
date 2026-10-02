"""The OpenTelemetry resource every signal carries."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

from opentelemetry.sdk.resources import Resource

from sideseat._version import __version__


def build(
    *,
    service_name: str,
    service_version: str,
    integrations: Sequence[str],
    extra: Mapping[str, Any],
) -> Resource:
    """The resource for this process.

    ``sideseat.framework`` names the primary integration. The current GenAI conventions are
    framework-neutral, so a producer that follows them emits nothing that says who it is; the server
    reads this declaration as a fallback when per-span evidence is absent.
    """
    attributes: dict[str, Any] = {
        "service.name": service_name,
        "service.version": service_version,
        "telemetry.sdk.name": "sideseat",
        "telemetry.sdk.language": "python",
        "telemetry.sdk.version": __version__,
    }
    if integrations:
        attributes["sideseat.framework"] = integrations[0]
        attributes["sideseat.integrations"] = list(integrations)
    attributes.update(extra)
    # Resource.create merges OTEL_RESOURCE_ATTRIBUTES underneath the explicit attributes.
    return Resource.create(attributes)
