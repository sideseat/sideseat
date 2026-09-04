"""OTEL resource creation."""

from opentelemetry.sdk.resources import Resource


def get_otel_resource(
    service_name: str, service_version: str, framework: str | None = None
) -> Resource:
    """Create OTEL resource with service info.

    ``framework`` is recorded as ``sideseat.framework`` when known, so the server does not have to
    guess what produced the spans. The current OTel GenAI conventions are framework-neutral by
    design - a producer that follows them emits ``gen_ai.*`` and nothing that says who it is - so a
    declaration is the only evidence for such spans. The server reads it as a *fallback*: per-span
    evidence still wins, so spans a nested library emits keep their own framework and this only
    fills the gap.
    """
    from sideseat._version import __version__ as sdk_version

    attributes = {
        "service.name": service_name,
        "service.version": service_version,
        "telemetry.sdk.name": "sideseat",
        "telemetry.sdk.version": sdk_version,
        "telemetry.sdk.language": "python",
    }
    if framework:
        attributes["sideseat.framework"] = framework
    return Resource.create(attributes)
