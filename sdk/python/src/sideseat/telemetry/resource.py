"""OTEL resource creation."""

from opentelemetry.sdk.resources import Resource


def get_otel_resource(service_name: str, service_version: str) -> Resource:
    """Create OTEL resource with service info."""
    from sideseat._version import __version__ as sdk_version

    return Resource.create(
        {
            "service.name": service_name,
            "service.version": service_version,
            "telemetry.sdk.name": "sideseat",
            "telemetry.sdk.version": sdk_version,
            "telemetry.sdk.language": "python",
        }
    )
